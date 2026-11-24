import test from 'node:test';
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
test('remote scheduled-task bridge strips private fields and preserves owning environment', () => {
  const result = spawnSync('python3', ['-c', `
import importlib.util, json
spec = importlib.util.spec_from_file_location('bridge', 'scripts/t3_bridge.py')
b = importlib.util.module_from_spec(spec)
spec.loader.exec_module(b)
class Fake:
    def call(self, name, args):
        assert name == 'list_scheduled_tasks'
        assert args == {'environmentId':'anon-env','projectId':'anon-project'}
        return {'tasks':[{'scheduledTaskId':'anon-task','title':'Anon job','enabled':True,
            'prompt':'PRIVATE-PROMPT','webhookUrl':'PRIVATE-TOKEN','webhookSecret':'PRIVATE-SECRET',
            'schedule':{'type':'webhook','signature':{'secret':'PRIVATE-SIGNATURE'},'filters':['PRIVATE-FILTER']}}]}
rows = b.list_peer_scheduled_tasks(Fake(), 'anon-env', {'anon-project':'Anon project'})
assert rows == [{'scheduledTaskId':'anon-task','title':'Anon job','enabled':True,
    'schedule':{'type':'webhook'},'project':'Anon project'}]
assert 'PRIVATE' not in json.dumps(rows)
class Broken:
    def call(self, name, args): raise RuntimeError('incomplete list')
try: b.list_peer_scheduled_tasks(Broken(), 'anon-env', {'anon-project':'Anon project'})
except RuntimeError: pass
else: raise AssertionError('partial listing must fail, not remove jobs')
`], { encoding: 'utf8' });
  assert.equal(result.status, 0, result.stderr);
});
