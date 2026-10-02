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
// commits, and stops the push.
//
// Stopping is not a choice. Git settles which commits a push sends before it runs
// this hook, so a commit made here can't join the push in flight; letting the push
// continue would send the unformatted tip and leave the fix behind.
func prePush(r repo, fs []formatter, stdin io.Reader, stderr io.Writer) (stopPush bool, err error) {
	pushed, err := pushedBranches(stdin)
	if err != nil {
		return false, err
	}
	if !describesPushedBranch(r, pushed) {
		return false, nil
	}

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

	// `--only` commits these paths and nothing else, whatever is staged. The
	// pre-commit hook has nothing left to do for files that were just formatted.
	if _, err := r.gitWithStdin(joinNul(formatted), "commit", "--quiet", "--no-verify", "--only",
		"-m", formatCommitSubject, "--pathspec-from-file=-", "--pathspec-file-nul"); err != nil {
		return false, err
	}
	sha, err := r.git("rev-parse", "--short", "HEAD")
	if err != nil {
		return false, err
	}
	fmt.Fprintf(stderr, "Formatted %d %s and committed the result as %s (%q).\n"+
		"Git can't add a commit to a push that's already running, so this push stopped. Push again to send everything.\n",
		len(formatted), pluralize(len(formatted), "file", "files"), strings.TrimSpace(sha), formatCommitSubject)
	return true, nil
}

// pushedBranches reads what git feeds a pre-push hook, one line per ref
// (`<local ref> <local sha> <remote ref> <remote sha>`), and returns the local
// branches being pushed. Tags and deletions are left out: neither puts new branch
// content on the remote.
func pushedBranches(stdin io.Reader) (map[string]bool, error) {
	branches := make(map[string]bool)
	scanner := bufio.NewScanner(stdin)
	for scanner.Scan() {
		fields := strings.Fields(scanner.Text())
		if len(fields) != 4 {
			continue
		}
		localRef, localSha := fields[0], fields[1]
		if strings.HasPrefix(localRef, "refs/heads/") && strings.Trim(localSha, "0") != "" {
			branches[localRef] = true
		}
	}
	return branches, scanner.Err()
}

// describesPushedBranch reports whether the working tree is a fair stand-in for
// what's being pushed. The formatters read files on disk, so their verdict only
// applies when the pushed branch is the one checked out, and a commit can only be
// added while no merge, rebase, or cherry-pick is underway.
func describesPushedBranch(r repo, pushed map[string]bool) bool {
	head, err := r.git("symbolic-ref", "--quiet", "HEAD")
	if err != nil || !pushed[strings.TrimSpace(head)] {
		return false
	}
	inProgress := []string{"MERGE_HEAD", "CHERRY_PICK_HEAD", "REVERT_HEAD", "rebase-merge", "rebase-apply"}
	return !slices.ContainsFunc(inProgress, r.gitPathExists)
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
