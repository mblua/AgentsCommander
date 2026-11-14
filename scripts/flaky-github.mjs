// #4527 P06: the ONLY flaky-rate module that starts processes. Every GitHub call goes
// through `gh api` with an argument array (never a shell string) and counts against an
// optional call budget.
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';

const MAX_BUFFER = 64 * 1024 * 1024;
const TIMEOUT_MS = 120000;

export class ApiError extends Error {
  constructor(endpoint, status, http) {
    super(`gh api ${endpoint} failed (exit ${status}${http ? `, HTTP ${http}` : ''})`);
    this.name = 'ApiError';
    this.endpoint = endpoint;
    this.status = status;
    this.http = http;
  }
}

export class BudgetExhausted extends Error {
  constructor(budget) {
    super(`GitHub API call budget of ${budget} reached`);
    this.name = 'BudgetExhausted';
  }
}

export function defaultRun(cmd, args, input) {
  const result = spawnSync(cmd, args, {
    input,
    encoding: 'buffer',
    maxBuffer: MAX_BUFFER,
    timeout: TIMEOUT_MS,
  });
  return { status: result.status, stdout: result.stdout, stderr: result.stderr };
}

function httpStatus(stderr) {
  const match = /HTTP (\d{3})/.exec(String(stderr ?? ''));
  return match ? Number(match[1]) : undefined;
}

export function createGitHub({ repo, run = defaultRun, budget = Infinity }) {
  const gh = { calls: 0 };

  function raw(endpoint, method, body) {
    if (gh.calls >= budget) throw new BudgetExhausted(budget);
    gh.calls += 1;
    const result = method
      ? run('gh', ['api', '--method', method, endpoint, '--input', '-'], JSON.stringify(body))
      : run('gh', ['api', endpoint]);
    if (result.status !== 0) throw new ApiError(endpoint, result.status, httpStatus(result.stderr));
    return result.stdout;
  }

  const json = (endpoint) => JSON.parse(String(raw(endpoint)));
  const send = (method, endpoint, body) => JSON.parse(String(raw(endpoint, method, body)));
  const actions = `repos/${repo}/actions`;
  const issues = `repos/${repo}/issues`;

  gh.listRuns = (workflowFile, day, page) =>
    json(
      `${actions}/workflows/${workflowFile}/runs?status=completed&created=${day}` +
        `&per_page=100&page=${page}`,
    );

  gh.attempt = (runId, n) => json(`${actions}/runs/${runId}/attempts/${n}`);

  gh.runArtifacts = (runId, page) =>
    json(`${actions}/runs/${runId}/artifacts?per_page=100&page=${page}`);

  gh.downloadText = (artifactId, member, tmpDir) => {
    const zip = path.join(tmpDir, `${artifactId}.zip`);
    fs.writeFileSync(zip, raw(`${actions}/artifacts/${artifactId}/zip`));
    try {
      const result = run('unzip', ['-p', zip, member]);
      return result.status === 0 ? String(result.stdout) : null;
    } finally {
      fs.rmSync(zip, { force: true });
    }
  };

  gh.latestHistory = (workflowFile, tmpDir) => {
    const runs = json(
      `${actions}/workflows/${workflowFile}/runs?branch=main&status=completed&per_page=5`,
    );
    for (const item of runs.workflow_runs ?? []) {
      const { artifacts = [] } = gh.runArtifacts(item.id, 1);
      const found = artifacts.find((a) => a.name === 'flaky-rate-history' && !a.expired);
      if (found) return gh.downloadText(found.id, 'history.json', tmpDir);
    }
    return null;
  };

  // #4527 P07: advisory quarantine issues. Bodies go as JSON on stdin, never in argv.
  // AC-4527-006: `more` comes from the raw page length, before pull requests are dropped.
  gh.listLabeledIssues = (label, page) => {
    const items = json(`${issues}?labels=${label}&state=all&per_page=100&page=${page}`);
    return { issues: items.filter((item) => !item.pull_request), more: items.length >= 100 };
  };

  gh.listComments = (number, page) =>
    json(`${issues}/${number}/comments?per_page=100&page=${page}`);

  gh.ensureLabel = (label) => {
    try {
      json(`repos/${repo}/labels/${label}`);
      return;
    } catch (error) {
      if (!(error instanceof ApiError) || error.http !== 404) throw error;
    }
    const body = { name: label, color: 'd93f0b', description: 'Flaky test (advisory, #4527)' };
    try {
      send('POST', `repos/${repo}/labels`, body);
    } catch (error) {
      if (!(error instanceof ApiError) || error.http !== 422) throw error;
    }
  };

  gh.createIssue = (title, body, labels) => send('POST', issues, { title, body, labels });
  gh.reopenIssue = (number) => send('PATCH', `${issues}/${number}`, { state: 'open' });
  gh.comment = (number, body) => send('POST', `${issues}/${number}/comments`, { body });

  return gh;
}
