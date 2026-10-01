/**
 * The sheet with S3 selected: no address, a provider preset that makes the endpoint,
 * the access key pair in the account fields, and refusals under the field that fixes
 * them. Plus the sign-in sheet's `access_keys` renderer.
 */

import { describe, expect, it, vi, beforeEach, afterEach } from 'vitest'
import { mount, tick } from 'svelte'
import SignInSheet from './SignInSheet.svelte'
import type { SignInAttemptOutcome, SignInSheetRequest, SignInSubmission } from './sign-in-contract'

vi.mock('$lib/tauri-commands', async (importOriginal) => ({
  ...(await importOriginal<Record<string, unknown>>()),
  notifyDialogOpened: vi.fn(() => Promise.resolve()),
  notifyDialogClosed: vi.fn(() => Promise.resolve()),
  listSavedServers: vi.fn(() => Promise.resolve([])),
}))

vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn(() => Promise.resolve(null)) }))

let submissions: SignInSubmission[] = []
/** What each round answers, in order; the last one repeats. */
let answers: SignInAttemptOutcome[] = []

beforeEach(() => {
  submissions = []
  answers = [{ kind: 'refused', refusal: 'unreachable' }]
})

afterEach(() => {
  document.body.innerHTML = ''
})

const attempt = (submission: SignInSubmission): Promise<SignInAttemptOutcome> => {
  submissions.push(submission)
  return Promise.resolve(answers.length > 1 ? (answers.shift() as SignInAttemptOutcome) : answers[0])
}

async function flush(times = 8) {
  for (let i = 0; i < times; i++) {
    await Promise.resolve()
    await tick()
  }
}

async function open(request: SignInSheetRequest) {
  mount(SignInSheet, {
    target: document.body.appendChild(document.createElement('div')),
    props: { request, onDone: () => {} },
  })
  await flush()
}

const field = (id: string) => document.body.querySelector<HTMLInputElement>(`#${id}`)

function type(id: string, value: string) {
  const input = field(id)
  if (!input) throw new Error(`no field #${id}`)
  input.value = value
  input.dispatchEvent(new Event('input', { bubbles: true }))
}

function press(label: string) {
  const button = [...document.body.querySelectorAll('button')].find((b) => b.textContent.trim() === label)
  if (!button) throw new Error(`no button saying ${label}`)
  button.click()
}

/** An S3 add on AWS, with the key and the region typed. */
async function s3OnAws(region = 'eu-west-1') {
  await open({ mode: 'add', attempt })
  press('S3')
  await tick()
  type('server-s3-region', region)
  type('server-username', 'AKIAEXAMPLE')
  type('server-secret', 's3cr3t')
  await tick()
}

function onlyTarget() {
  const last = submissions.at(-1)
  if (last?.mode !== 'add') throw new Error('expected an add submission')
  return last
}

describe('SignInSheet: the S3 form', () => {
  it('asks for a provider, its region, a bucket, and the key pair, and no address', async () => {
    await s3OnAws()
    expect(field('server-address')).toBeNull()
    expect(document.body.querySelector('label[for="server-username"]')?.textContent).toBe('Access key ID')
    expect(document.body.querySelector('label[for="server-secret"]')?.textContent).toBe('Secret access key')
    expect(field('server-secret')?.getAttribute('autocomplete')).toBe('off')
    // An empty bucket opens the whole account, and the field says so.
    expect(document.body.querySelector('#server-s3-bucket-help')?.textContent).toContain('every bucket')
  })

  it('sends the AWS preset, the key, the account root for an empty bucket, and the secret', async () => {
    await s3OnAws()
    press('Add and open')
    await flush()
    expect(onlyTarget()).toEqual({
      mode: 'add',
      target: {
        protocol: 's3',
        displayName: '',
        provider: { kind: 'aws', region: 'eu-west-1' },
        accessKeyId: 'AKIAEXAMPLE',
        bucket: null,
        autoReconnect: true,
      },
      secret: { secret: 's3cr3t', remember: true },
      intent: 'open',
    })
  })

  it('refuses a region no host name can carry under the region, before dialing', async () => {
    await s3OnAws('EU West 1')
    press('Add and open')
    await flush()
    expect(submissions).toEqual([])
    expect(document.body.querySelector('#server-s3-zone-refusal')?.textContent).toContain('lowercase letters')
    expect(document.activeElement).toBe(field('server-s3-region'))
  })

  it('puts a missing bucket under the bucket field', async () => {
    answers = [{ kind: 'refused', refusal: 'bucket_not_found' }]
    await s3OnAws()
    type('server-s3-bucket', 'nope')
    await tick()
    press('Add and open')
    await flush()
    expect(document.body.querySelector('#server-s3-bucket-refusal')?.textContent).toContain(
      's3.eu-west-1.amazonaws.com',
    )
    expect(document.activeElement).toBe(field('server-s3-bucket'))
  })

  it('names the region a bucket lives in, and one press switches to it and tries again', async () => {
    answers = [{ kind: 'refused', refusal: 'region_mismatch', region: 'us-east-2' }, { kind: 'cancelled' }]
    await s3OnAws()
    press('Add and open')
    await flush()
    expect(document.body.querySelector('#server-s3-zone-refusal')?.textContent).toContain('us-east-2')

    press('Use us-east-2')
    await flush()
    expect(submissions).toHaveLength(2)
    expect(onlyTarget().target).toMatchObject({ provider: { kind: 'aws', region: 'us-east-2' } })
  })
})

describe('SignInSheet: the access_keys renderer', () => {
  it('shows the access key ID as the read-only account, over one secret access key field', async () => {
    await open({
      mode: 'sign-in',
      remembered: false,
      endpoint: {
        protocol: 's3',
        displayName: 'photos',
        address: 's3.eu-west-1.amazonaws.com/photos',
        host: 's3.eu-west-1.amazonaws.com',
        username: 'AKIAEXAMPLE',
      },
      shape: { kind: 'access_keys' },
      attempt,
    })
    expect(document.body.querySelector('#sign-in-username-label')?.textContent).toBe('Access key ID')
    expect(document.body.querySelector('#sign-in-username')?.textContent).toBe('AKIAEXAMPLE')
    expect(document.body.querySelector('label[for="sign-in-secret"]')?.textContent).toBe('Secret access key')
    expect(field('sign-in-secret')?.getAttribute('autocomplete')).toBe('off')

    answers = [{ kind: 'refused', refusal: 'authentication_rejected' }]
    type('sign-in-secret', 'wrong')
    await tick()
    press('Sign in')
    await flush()
    // ❗ The key ID is the volume's identity, so the round sends no username.
    expect(submissions).toEqual([{ mode: 'sign-in', secret: { secret: 'wrong', remember: false }, username: null }])
    expect(document.body.querySelector('#sign-in-secret-refusal')?.textContent).toBe(
      'That secret access key didn’t work for AKIAEXAMPLE.',
    )
  })
})
