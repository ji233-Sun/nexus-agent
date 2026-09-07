import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import test from 'node:test';
import {
  applyResolutions, isCommand, parseConflicts, prepareRequest, publishResolution,
  requestResolution, resolveConflicts, verifyCandidate,
} from './resolve-conflicts.mjs';

const EVENT = {
  action: 'created',
  repository: { full_name: 'owner/repo', default_branch: 'main' },
  issue: { number: 7, pull_request: {} },
  comment: { body: '/resolve-conflicts', user: { login: 'maintainer', type: 'User' } },
};

function git(repo, ...args) {
  return execFileSync('git', ['-c', 'core.hooksPath=/dev/null', ...args], {
    cwd: repo, encoding: 'utf8', stdio: 'pipe',
  }).trim();
}

function fixture(t, { base = 'value = 1;\n', ours = 'value = 2;\n', theirs = 'value = 3;\n', extra = {} } = {}) {
  const root = mkdtempSync(join(tmpdir(), 'nexus-conflicts-test-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const repo = join(root, 'repo');
  mkdirSync(repo);
  git(repo, 'init', '-b', 'main');
  git(repo, 'config', 'user.name', 'Test');
  git(repo, 'config', 'user.email', 'test@example.invalid');
  git(repo, 'config', 'core.autocrlf', 'false');
  const file = join(repo, 'example.txt');
  writeFileSync(file, base);
  git(repo, 'add', '.');
  git(repo, 'commit', '-m', 'base');
  git(repo, 'switch', '-c', 'feat/example');
  if (ours === null) {
    git(repo, 'rm', 'example.txt');
  } else {
    writeFileSync(file, ours);
  }
  for (const [name, content] of Object.entries(extra)) {
    mkdirSync(dirname(join(repo, name)), { recursive: true });
    writeFileSync(join(repo, name), content);
  }
  git(repo, 'add', '.');
  git(repo, 'commit', '--allow-empty', '-m', 'PR change');
  const headSha = git(repo, 'rev-parse', 'HEAD');
  git(repo, 'switch', 'main');
  writeFileSync(file, theirs);
  git(repo, 'add', '.');
  git(repo, 'commit', '--allow-empty', '-m', 'target change');
  const baseSha = git(repo, 'rev-parse', 'HEAD');
  git(repo, 'switch', 'feat/example');
  return { root, repo, headSha, baseSha, bundlePath: join(root, 'candidate.bundle') };
}

function prFor(f) {
  return {
    state: 'open', draft: false,
    head: { sha: f.headSha, ref: 'feat/example', repo: { full_name: 'owner/repo' } },
    base: { sha: f.baseSha, ref: 'main' },
  };
}

function apiFor(pr, calls, permission = 'write') {
  return async (path, options = {}) => {
    calls.push({ path, ...options });
    if (path.endsWith('/permission')) return { permission };
    if (path.endsWith('/pulls/7')) return structuredClone(pr);
    return {};
  };
}

const answer = (content) => ({ resolutions: [{ id: 0, content }] });

test('only exact human PR commands can reach the permission check', async () => {
  assert.equal(isCommand(EVENT), true);
  for (const variant of [
    { ...EVENT, action: 'edited' },
    { ...EVENT, issue: { number: 7 } },
    { ...EVENT, comment: { ...EVENT.comment, body: '/resolve-conflicts && echo unsafe' } },
    { ...EVENT, comment: { ...EVENT.comment, user: { login: 'bot', type: 'Bot' } } },
  ]) {
    assert.equal(isCommand(variant), false);
    const result = await prepareRequest(variant, () => assert.fail('must not contact GitHub'));
    assert.equal(result.authorized, false);
  }
});

test('current write permission is required; fork, draft, closed and non-default-base PRs are skipped', async () => {
  const pr = prFor({ headSha: 'a'.repeat(40), baseSha: 'b'.repeat(40) });
  const calls = [];
  assert.equal((await prepareRequest(EVENT, apiFor(pr, calls, 'read'))).authorized, false);
  assert.equal(calls.length, 1);
  for (const variant of [
    { ...pr, state: 'closed' },
    { ...pr, draft: true },
    { ...pr, head: { ...pr.head, repo: { full_name: 'someone/fork' } } },
    { ...pr, base: { ...pr.base, ref: 'release' } },
  ]) {
    const result = await prepareRequest(EVENT, apiFor(variant, []));
    assert.equal(result.authorized, true);
    assert.equal(result.eligible, false);
  }
  assert.equal((await prepareRequest(EVENT, apiFor(pr, []))).eligible, true);
});

test('a real merge preserves both parents and all text outside the conflict, including BOM and CRLF', async (t) => {
  const common = ['one', 'two', 'three', 'four', 'five'].join('\r\n');
  const f = fixture(t, {
    base: `\ufeffheader\r\n${common}\r\nvalue = 1;\r\n${common}\r\nfooter\r\n`,
    ours: `\ufeffPR header\r\n${common}\r\nvalue = 2;\r\n${common}\r\nfooter\r\n`,
    theirs: `\ufeffheader\r\n${common}\r\nvalue = 3;\r\n${common}\r\ntarget footer\r\n`,
  });
  let requests = 0;
  const result = await resolveConflicts(f.repo, f.headSha, f.baseSha, f.bundlePath, async (file, text, spans) => {
    requests += 1;
    assert.equal(file, 'example.txt');
    assert.equal(spans.length, 1);
    assert.ok(text.includes('PR header'));
    assert.ok(text.includes('target footer'));
    return answer('value = 5;\r\n');
  });
  assert.equal(requests, 1);
  assert.equal(result.outcome, 'resolved');
  assert.equal(readFileSync(join(f.repo, 'example.txt'), 'utf8'),
    `\ufeffPR header\r\n${common}\r\nvalue = 5;\r\n${common}\r\ntarget footer\r\n`);
  assert.equal(git(f.repo, 'rev-list', '--parents', '-n', '1', 'HEAD'),
    `${result.candidate_sha} ${f.headSha} ${f.baseSha}`);
  assert.equal(git(f.repo, 'status', '--porcelain'), '');
  assert.ok(existsSync(f.bundlePath));
  verifyCandidate(f.repo, f.bundlePath, result.candidate_sha, f.headSha, f.baseSha);
  assert.throws(() => verifyCandidate(f.repo, f.bundlePath, '0'.repeat(40), f.headSha, f.baseSha));
});

test('conflict-free merges do not call the model or create a commit', async (t) => {
  const f = fixture(t, { ours: 'value = 1;\n' });
  const result = await resolveConflicts(f.repo, f.headSha, f.baseSha, f.bundlePath,
    () => assert.fail('model must not run'));
  assert.equal(result.outcome, 'no_conflicts');
  assert.equal(git(f.repo, 'rev-parse', 'HEAD'), f.headSha);
  assert.equal(existsSync(f.bundlePath), false);
});

test('unsupported conflicts and workflow changes stop before any model calls', async (t) => {
  for (const options of [
    { base: Buffer.from([0, 1]), ours: Buffer.from([0, 2]), theirs: Buffer.from([0, 3]) },
    { ours: null },
    { extra: { '.github/workflows/ci.yml': 'untrusted workflow' } },
  ]) {
    const f = fixture(t, options);
    await assert.rejects(resolveConflicts(f.repo, f.headSha, f.baseSha, f.bundlePath,
      () => assert.fail('model must not run')),
    (error) => ['unsupported_conflict', 'workflow_changes'].includes(error.code));
    assert.equal(git(f.repo, 'rev-parse', 'HEAD'), f.headSha);
    assert.equal(existsSync(f.bundlePath), false);
  }
});

test('invalid model output cannot produce a candidate commit', async (t) => {
  const f = fixture(t);
  await assert.rejects(resolveConflicts(f.repo, f.headSha, f.baseSha, f.bundlePath,
    async () => ({ resolutions: null })), { code: 'invalid_answer' });
  assert.equal(git(f.repo, 'rev-parse', 'HEAD'), f.headSha);
  assert.equal(existsSync(f.bundlePath), false);
});

test('replacement validation rejects missing, duplicate, unterminated and marker-containing blocks', () => {
  const block = `${'<'.repeat(32)} ours\nours\n${'|'.repeat(32)} base\nbase\n${'='.repeat(32)}\ntheirs\n${'>'.repeat(32)} theirs\n`;
  const text = `before\n${block}between\n${block}after\n`;
  const spans = parseConflicts(text);
  assert.equal(applyResolutions(text, spans, { resolutions: [
    { id: 0, content: 'first\n' }, { id: 1, content: 'second\n' },
  ] }), 'before\nfirst\nbetween\nsecond\nafter\n');
  for (const replacements of [
    [{ id: 0, content: 'first\n' }],
    [{ id: 0, content: 'first\n' }, { id: 0, content: 'duplicate\n' }],
    [{ id: 0, content: 'first' }, { id: 1, content: 'second\n' }],
    [{ id: 0, content: '<<<<<<< ours\n' }, { id: 1, content: 'second\n' }],
  ]) {
    assert.throws(() => applyResolutions(text, spans, { resolutions: replacements }), { code: 'invalid_answer' });
  }
  assert.throws(() => parseConflicts(`${'<'.repeat(32)} ours\nunfinished\n`), { code: 'unsupported_conflict' });
});

test('DeepSeek requests use JSON output and reject truncated or empty completions', async (t) => {
  const previous = process.env.DEEPSEEK_API_KEY;
  process.env.DEEPSEEK_API_KEY = 'test-key';
  t.after(() => {
    if (previous === undefined) delete process.env.DEEPSEEK_API_KEY;
    else process.env.DEEPSEEK_API_KEY = previous;
  });
  const valid = answer('resolved\n');
  const result = await requestResolution('file.txt', 'conflicted text', [{}], async (url, options) => {
    assert.equal(url, 'https://api.deepseek.com/chat/completions');
    const body = JSON.parse(options.body);
    assert.equal(body.model, 'deepseek-v4-flash');
    assert.equal(body.response_format.type, 'json_object');
    assert.equal(body.tools, undefined);
    return Response.json({ choices: [{ finish_reason: 'stop', message: { content: JSON.stringify(valid) } }] });
  });
  assert.deepEqual(result, valid);
  for (const choice of [
    { finish_reason: 'length', message: { content: '{"resolutions":[' } },
    { finish_reason: 'stop', message: { content: '' } },
  ]) {
    await assert.rejects(requestResolution('file.txt', '', [], async () => Response.json({ choices: [choice] })),
      { code: 'invalid_answer' });
  }
  delete process.env.DEEPSEEK_API_KEY;
  await assert.rejects(requestResolution('file.txt', '', [], () => assert.fail('must not call API')),
    { code: 'missing_key' });
});

test('publishing a verified merge updates only the PR branch, then explicitly starts CI', async (t) => {
  const f = fixture(t);
  const remote = join(f.root, 'remote.git');
  git(f.root, 'clone', '--bare', f.repo, remote);
  const result = await resolveConflicts(f.repo, f.headSha, f.baseSha, f.bundlePath, async () => answer('value = 5;\n'));
  const publisher = join(f.root, 'publisher');
  git(f.root, 'clone', remote, publisher);
  const calls = [];
  const api = apiFor(prFor(f), calls);
  await publishResolution(EVENT, publisher, {
    outcome: 'resolved', ...f, candidateSha: result.candidate_sha,
  }, async (path, options) => {
    if (path.endsWith('/dispatches')) {
      assert.equal(git(remote, 'rev-parse', 'refs/heads/feat/example'), result.candidate_sha);
    }
    return api(path, options);
  });
  assert.equal(git(remote, 'rev-parse', 'refs/heads/main'), f.baseSha);
  assert.equal(git(publisher, 'rev-parse', 'HEAD'), f.headSha);
  assert.deepEqual(calls.find(({ path }) => path.endsWith('/dispatches')).body, { ref: 'feat/example' });
  assert.match(calls.at(-1).body.body, /已触发现有 CI/);
});

test('stale PR or base commits are reported without pushing or dispatching CI', async (t) => {
  const f = fixture(t);
  for (const side of ['head', 'base']) {
    const pr = prFor(f);
    pr[side].sha = 'f'.repeat(40);
    const calls = [];
    await assert.rejects(publishResolution(EVENT, f.repo, {
      outcome: 'resolved', ...f, candidateSha: 'e'.repeat(40),
    }, apiFor(pr, calls)), { code: 'publish_error' });
    assert.match(calls.at(-1).body.body, /已停止写回/);
    assert.equal(calls.some(({ path }) => path.endsWith('/dispatches')), false);
  }
});

test('a concurrent commit arriving after the API check is never overwritten by the push', async (t) => {
  const f = fixture(t);
  const remote = join(f.root, 'remote.git');
  git(f.root, 'clone', '--bare', f.repo, remote);
  const result = await resolveConflicts(f.repo, f.headSha, f.baseSha, f.bundlePath, async () => answer('value = 5;\n'));
  const publisher = join(f.root, 'publisher');
  git(f.root, 'clone', remote, publisher);
  const writer = join(f.root, 'writer');
  git(f.root, 'clone', remote, writer);
  writeFileSync(join(writer, 'human.txt'), 'new work\n');
  git(writer, 'add', 'human.txt');
  git(writer, '-c', 'user.name=Test', '-c', 'user.email=test@example.invalid', 'commit', '-m', 'concurrent change');
  const humanSha = git(writer, 'rev-parse', 'HEAD');
  git(writer, 'push', 'origin', 'HEAD:refs/heads/feat/example');
  const calls = [];
  // The API snapshot predates the writer's push; Git must still reject the stale candidate.
  await assert.rejects(publishResolution(EVENT, publisher, {
    outcome: 'resolved', ...f, candidateSha: result.candidate_sha,
  }, apiFor(prFor(f), calls)), { code: 'publish_error' });
  assert.equal(git(remote, 'rev-parse', 'refs/heads/feat/example'), humanSha);
  assert.equal(calls.some(({ path }) => path.endsWith('/dispatches')), false);
  assert.match(calls.at(-1).body.body, /未强制覆盖 PR 分支/);
});

test('a failed CI dispatch is reported as a pushed commit, not as a failed push', async (t) => {
  const f = fixture(t);
  const remote = join(f.root, 'remote.git');
  git(f.root, 'clone', '--bare', f.repo, remote);
  const result = await resolveConflicts(f.repo, f.headSha, f.baseSha, f.bundlePath, async () => answer('value = 5;\n'));
  const publisher = join(f.root, 'publisher');
  git(f.root, 'clone', remote, publisher);
  const calls = [];
  const api = apiFor(prFor(f), calls);
  await assert.rejects(publishResolution(EVENT, publisher, {
    outcome: 'resolved', ...f, candidateSha: result.candidate_sha,
  }, async (path, options) => {
    if (path.endsWith('/dispatches')) throw new Error('dispatch unavailable');
    return api(path, options);
  }), { code: 'publish_error' });
  assert.equal(git(remote, 'rev-parse', 'refs/heads/feat/example'), result.candidate_sha);
  assert.match(calls.at(-1).body.body, /提交已写回，但 CI 启动失败/);
});
