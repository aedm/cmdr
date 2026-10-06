package main

import (
	"bufio"
	"fmt"
	"io"
	"slices"
	"strings"
	"sync"
)

const formatCommitSubject = "style: apply formatter output"

// prePush is the backstop for what pre-commit never sees: a rebase, a conflict
// resolution, a `--no-verify` commit, a generated file. It runs the same
// whole-repo checks as CI's formatting lanes, and when they'd fail it formats,
// commits, and stops the push. Then it does the same for the license notices a
// dependency push makes stale, and runs CI's size limits (`prepush_checks.go`).
//
// Stopping is not a choice. Git settles which commits a push sends before it runs
// this hook, so a commit made here can't join the push in flight; letting the push
// continue would send the unformatted tip and leave the fix behind.
func prePush(r repo, fs []formatter, check checkRunner, stdin io.Reader, stderr io.Writer) (stopPush bool, err error) {
	pushed, err := pushedBranches(stdin)
	if err != nil {
		return false, err
	}
	ref, ok := describesPushedBranch(r, pushed)
	if !ok {
		return false, nil
	}

	if stop, err := commitFormatting(r, fs, stderr); stop || err != nil {
		return stop, err
	}
	if stop, err := regenerateNotices(r, check, ref, stderr); stop || err != nil {
		return stop, err
	}
	return checkBudgets(r, check, stderr)
}

// commitFormatting formats what CI's formatting lanes would flag, commits it, and
// reports whether it did (which stops the push).
func commitFormatting(r repo, fs []formatter, stderr io.Writer) (stopPush bool, err error) {
	fixable, err := fixableFiles(r, unformattedFiles(fs, stderr))
	if err != nil || len(fixable) == 0 {
		return false, err
	}
	for _, f := range fs {
		if err := f.format(fixable); err != nil {
			fmt.Fprintf(stderr, "format hook: %v\n", err)
		}
	}
	// The fixable files matched the index, so the ones that differ now are the ones
	// a formatter rewrote. If none did, a check and its formatter disagree, and
	// stopping the push would stop every retry as well.
	modified, err := r.paths("diff", "--name-only", "-z")
	if err != nil {
		return false, err
	}
	formatted := intersect(fixable, toSet(modified))
	if len(formatted) == 0 {
		return false, nil
	}

	sha, err := commitOnly(r, formatted, formatCommitSubject)
	if err != nil {
		return false, err
	}
	fmt.Fprintf(stderr, "Formatted %d %s and committed the result as %s (%q).\n"+
		"Git can't add a commit to a push that's already running, so this push stopped. Push again to send everything.\n",
		len(formatted), pluralize(len(formatted), "file", "files"), sha, formatCommitSubject)
	return true, nil
}

// commitOnly commits these paths and nothing else, whatever is staged (`--only`),
// and returns the new commit's short sha. The pre-commit hook has nothing left to
// do for files a hook just rewrote, so it's skipped.
func commitOnly(r repo, paths []string, subject string) (string, error) {
	if _, err := r.gitWithStdin(joinNul(paths), "commit", "--quiet", "--no-verify", "--only",
		"-m", subject, "--pathspec-from-file=-", "--pathspec-file-nul"); err != nil {
		return "", err
	}
	sha, err := r.git("rev-parse", "--short", "HEAD")
	return strings.TrimSpace(sha), err
}

// pushedRef is one branch in a push: the local tip being sent and the remote tip
// it replaces (all zeros for a new branch).
type pushedRef struct {
	localSha, remoteSha string
}

// pushedBranches reads what git feeds a pre-push hook, one line per ref
// (`<local ref> <local sha> <remote ref> <remote sha>`), and returns the local
// branches being pushed. Tags and deletions are left out: neither puts new branch
// content on the remote.
func pushedBranches(stdin io.Reader) (map[string]pushedRef, error) {
	branches := make(map[string]pushedRef)
	scanner := bufio.NewScanner(stdin)
	for scanner.Scan() {
		fields := strings.Fields(scanner.Text())
		if len(fields) != 4 {
			continue
		}
		localRef, localSha, remoteSha := fields[0], fields[1], fields[3]
		if strings.HasPrefix(localRef, "refs/heads/") && strings.Trim(localSha, "0") != "" {
			branches[localRef] = pushedRef{localSha: localSha, remoteSha: remoteSha}
		}
	}
	return branches, scanner.Err()
}

// describesPushedBranch reports whether the working tree is a fair stand-in for
// what's being pushed, and returns the checked-out branch's push. The formatters
// read files on disk, so their verdict only applies when the pushed branch is the
// one checked out, and a commit can only be added while no merge, rebase, or
// cherry-pick is underway.
func describesPushedBranch(r repo, pushed map[string]pushedRef) (pushedRef, bool) {
	head, err := r.git("symbolic-ref", "--quiet", "HEAD")
	if err != nil {
		return pushedRef{}, false
	}
	ref, ok := pushed[strings.TrimSpace(head)]
	if !ok {
		return pushedRef{}, false
	}
	inProgress := []string{"MERGE_HEAD", "CHERRY_PICK_HEAD", "REVERT_HEAD", "rebase-merge", "rebase-apply"}
	return ref, !slices.ContainsFunc(inProgress, r.gitPathExists)
}

// unformattedFiles asks every formatter, in parallel, what its CI lane would flag.
// A formatter that can't answer is reported and skipped.
func unformattedFiles(fs []formatter, stderr io.Writer) []string {
	results := make([][]string, len(fs))
	errs := make([]error, len(fs))
	var wg sync.WaitGroup
	for i, f := range fs {
		wg.Go(func() {
			results[i], errs[i] = f.unformatted()
		})
	}
	wg.Wait()

	var files []string
	for i, f := range fs {
		if errs[i] != nil {
			fmt.Fprintf(stderr, "format hook: skipped %s, %v\n", f.name(), errs[i])
			continue
		}
		files = append(files, results[i]...)
	}
	return files
}

// fixableFiles narrows unformatted files to those the hook may rewrite and commit:
// tracked, and identical to the commit being pushed. A file with uncommitted
// changes is someone's work in progress; formatting it says nothing about the
// pushed commit, and committing it would sweep that work along. Untracked files
// aren't part of the push at all.
func fixableFiles(r repo, unformatted []string) ([]string, error) {
	if len(unformatted) == 0 {
		return nil, nil
	}
	tracked, err := r.paths("ls-files", "-z")
	if err != nil {
		return nil, err
	}
	changedInWorktree, err := r.paths("diff", "--name-only", "-z", "HEAD")
	if err != nil {
		return nil, err
	}
	changedInIndex, err := r.paths("diff", "--cached", "--name-only", "-z")
	if err != nil {
		return nil, err
	}
	isTracked, inWorktree, inIndex := toSet(tracked), toSet(changedInWorktree), toSet(changedInIndex)

	var fixable []string
	for _, p := range unformatted {
		if isTracked[p] && !inWorktree[p] && !inIndex[p] {
			fixable = append(fixable, p)
		}
	}
	return fixable, nil
}
