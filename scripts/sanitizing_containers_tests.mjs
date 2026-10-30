import test from 'node:test';
import assert from 'node:assert/strict';
import { parseRegistry, runGate as containersGate } from './sanitizing_containers.mjs';

const attemptsScope = 'src-tauri/src/' + 'cli/role_experiment/';
const attemptsRegistry = () => ({
  version: 1,
  containers: [{
    id: 'RoleExperiment/Attempts', name: 'attempts.jsonl', scope: attemptsScope,
    owner: attemptsScope + 'storage.rs', externalLiteralBaseline: 0,
    writerInventory: { files: 0, sites: 0 },
  }],
});
function containerTree(files = {}, registry = attemptsRegistry()) {
  const blobs = {
    'scripts/containers.json': JSON.stringify(registry),
    [attemptsScope + 'storage.rs']: 'const NAME: &str = "attempts.jsonl";',
    ...files,
  };
  const headTree = new Map(Object.keys(blobs).map(path => [path, path]));
  return { headTree, readBlob: entry => blobs[entry], assertStableHead() {} };
}
test('04 accepts safe basenames and enforces their scoped owner', () => {
  for (const name of [
    'config.json', 'attempts.jsonl', '.team-config-write.lock', 'conventions.md',
    'Role.md', 'TASK.md', 'README', 'a_b-1.txt',
  ]) {
    const registry = attemptsRegistry();
    registry.containers[0].name = name;
    assert.equal(parseRegistry(JSON.stringify(registry))[0].name, name);
    const literal = 'const NAME: &str = "' + name + '";';
    const files = { [attemptsScope + 'storage.rs']: literal };
    assert.equal(containersGate(containerTree(files, registry)).exit, 0, name);
    files[attemptsScope + 'resume.rs'] = literal;
    const result = containersGate(containerTree(files, registry));
    assert.equal(result.exit, 1, name);
    assert.ok(result.diagnostics.some(d => d.rule === 'external_container_literal'), name);
  }
});
test('04 rejects unsafe basenames and nonstrings', () => {
  for (const name of [
    '', '.', '..', '../Role.md', './Role.md', 'a/b', 'a\b', 'C:Role.md',
    'a b', ' a', 'a ', '\n', 'a\n', 'a:b', 'a*', 'é.md', null, 1, true, {}, [],
  ]) {
    const registry = attemptsRegistry();
    registry.containers[0].name = name;
    assert.throws(() => parseRegistry(JSON.stringify(registry)), String(name));
    assert.equal(containersGate(containerTree({}, registry)).status, 'ERROR');
  }
});
