"""Scoped MCP transport for Skywatch. JSON on stdin/stdout; never logs credentials."""
import json
import os
from pathlib import Path
import sys
import urllib.error
import urllib.parse
import urllib.request

# T3 listens on this machine. Corporate proxies (HTTP_PROXY or Windows
# Internet Settings) often don't exempt 127.0.0.1, so loopback goes direct.
LOOPBACK = {'127.0.0.1', 'localhost', '::1'}
# Wildcard bind addresses work as a destination on Linux but not on Windows.
WILDCARD = {'0.0.0.0': '127.0.0.1', '::': '[::1]'}


def local_url(url):
    """Point a T3 origin that names a wildcard bind address at loopback."""
    parts = urllib.parse.urlsplit(url)
    if parts.hostname in WILDCARD:
        port = ':' + str(parts.port) if parts.port else ''
        parts = parts._replace(netloc=WILDCARD[parts.hostname] + port)
    return urllib.parse.urlunsplit(parts)


def opener(url):
    if urllib.parse.urlsplit(url).hostname in LOOPBACK:
        return urllib.request.build_opener(urllib.request.ProxyHandler({}))
    return urllib.request.build_opener()


def decode(raw):
    text = raw.decode()
    if text.lstrip().startswith('{'):
        return json.loads(text)
    for line in text.splitlines():
        if line.startswith('data:'):
            value = json.loads(line[5:].strip())
            if 'result' in value or 'error' in value:
                return value
    raise ValueError('MCP response contains no result')


class Client:
    def __init__(self):
        self.endpoint = os.environ.get('SKYWATCH_T3_MCP_URL')
        if not self.endpoint:
            home = Path(os.environ.get('T3CODE_HOME', str(Path.home() / '.t3')))
            runtime = json.loads((home / 'userdata' / 'server-runtime.json').read_text(encoding='utf-8'))
            self.endpoint = runtime['origin'].rstrip('/') + '/mcp'
        self.endpoint = local_url(self.endpoint)
        self.opener = opener(self.endpoint)
        token_file = os.environ.get('SKYWATCH_T3_TOKEN_FILE', str(Path.home() / '.config' / 'skywatch' / 't3-mcp-token'))
        if not token_file:
            raise ValueError('SKYWATCH_T3_TOKEN_FILE must name a paired MCP client credential')
        self.token = Path(token_file).read_text(encoding='utf-8').strip()
        self.session = None
        self.rpc('initialize', {'protocolVersion': '2025-06-18', 'capabilities': {},
                 'clientInfo': {'name': 'skywatch', 'version': '1'}})
        self.rpc('notifications/initialized', {}, notification=True)

    def rpc(self, method, params, notification=False):
        body = {'jsonrpc': '2.0', 'method': method, 'params': params}
        if not notification:
            body['id'] = 1
        headers = {'Content-Type': 'application/json', 'Accept': 'application/json, text/event-stream',
                   'Authorization': 'Bearer ' + self.token, 'MCP-Protocol-Version': '2025-06-18'}
        if self.session:
            headers['Mcp-Session-Id'] = self.session
        request = urllib.request.Request(self.endpoint, json.dumps(body).encode(), headers)
        with self.opener.open(request, timeout=8) as response:
            self.session = response.headers.get('Mcp-Session-Id', self.session)
            raw = response.read()
        if notification:
            return None
        result = decode(raw)
        if 'error' in result:
            raise ValueError('MCP request rejected')
        return result['result']

    def call(self, name, arguments):
        result = self.rpc('tools/call', {'name': name, 'arguments': arguments})
        if result.get('isError'):
            raise ValueError('T3 tool rejected request')
        if 'structuredContent' in result:
            return result['structuredContent']
        return json.loads(next(c['text'] for c in result['content'] if c['type'] == 'text'))


def list_projects(client, environment_id):
    """Project id -> title; raises if the listing is incomplete."""
    projects = {}
    cursor = None
    while True:
        args = {'environmentId': environment_id, 'limit': 100}
        if cursor is not None:
            args['cursor'] = cursor
        page = client.call('t3_project_list', args)
        for proj in page.get('projects', []):
            if proj.get('id'):
                projects[proj['id']] = (proj.get('title') or '').strip()
        cursor = page.get('nextCursor')
        if cursor is None:
            return projects


# Schedule trigger fields Skywatch may show. Webhook filters, signatures,
# secrets and dedupe keys are dropped before anything leaves this process.
SCHEDULE_FIELDS = ('type', 'everyMs', 'timeOfDay', 'weekdays', 'runAt')
# Task fields Skywatch may show. The prompt never leaves this process.
TASK_FIELDS = ('scheduledTaskId', 'title', 'enabled', 'boundThreadId', 'nextRunAt',
               'lastRunAt', 'lastRunStatus', 'runCount')


def safe_task(task, project_title):
    row = {key: task[key] for key in TASK_FIELDS if key in task}
    schedule = task.get('schedule') if isinstance(task.get('schedule'), dict) else {}
    row['schedule'] = {key: schedule[key] for key in SCHEDULE_FIELDS if key in schedule}
    if project_title:
        row['project'] = project_title
    return row


def list_peer_scheduled_tasks(client, environment_id, projects):
    """Read-only listing of every project's scheduled tasks, or None if any
    project could not be listed (a partial list must not look like deletions)."""
    rows = []
    for project_id, title in projects.items():
        page = client.call('list_scheduled_tasks', {'environmentId': environment_id, 'projectId': project_id})
        for task in page.get('tasks', []):
            if isinstance(task, dict) and task.get('scheduledTaskId'):
                rows.append(safe_task(task, title))
    return rows


def list_peer_threads(client, environment_id, projects):
    projects = {pid: title for pid, title in (projects or {}).items() if title}

    rows = []
    seen = set()

    def attach(threads):
        for thread in threads:
            tid = thread.get('threadId')
            if not tid or tid in seen:
                continue
            seen.add(tid)
            if not thread.get('project') and thread.get('projectId') in projects:
                thread = dict(thread)
                thread['project'] = projects[thread['projectId']]
            rows.append(thread)

    # Fetch all unsettled threads (typically 1 fast page), including active subagents.
    cursor = 0
    while cursor is not None:
        page = client.call('t3_thread_list', {
            'environmentId': environment_id,
            'allProjects': True,
            'settled': False,
            'includeSubagents': True,
            'limit': 100,
            'cursor': cursor,
        })
        attach(page.get('threads', []))
        cursor = page.get('nextCursor')

    # Fetch the most recent settled page so freshly completed/settled threads trigger departure.
    settled_page = client.call('t3_thread_list', {
        'environmentId': environment_id,
        'allProjects': True,
        'settled': True,
        'includeSubagents': False,
        'limit': 50,
        'cursor': 0,
    })
    attach(settled_page.get('threads', []))
    return rows


def main():
    request = json.load(sys.stdin.buffer)
    client = Client()
    if request.get('action') == 'send':
        return client.call('t3_thread_send', request['arguments'])
    peers = client.call('t3_environment_links', {}).get('links', [])
    result = []
    for peer in peers:
        item = {'environment_id': peer['environmentId'], 'label': peer['label'],
                'online': peer['status'] == 'reachable', 'threads': None,
                'scheduled_tasks': None}
        if item['online']:
            try:
                projects = list_projects(client, peer['environmentId'])
            except Exception:
                projects = None
            try:
                item['threads'] = list_peer_threads(client, peer['environmentId'], projects)
            except Exception:
                item['online'] = False
            if item['online'] and projects is not None and request.get('scheduledTasks'):
                try:
                    item['scheduled_tasks'] = list_peer_scheduled_tasks(client, peer['environmentId'], projects)
                except Exception:
                    pass
        result.append(item)
    return result


if __name__ == '__main__':
    try:
        print(json.dumps(main()))
    except Exception as error:
        reason = type(error).__name__
        if isinstance(error, urllib.error.HTTPError):
            reason += ' (HTTP ' + str(error.code) + '; re-run scripts/pair_t3.py if the token expired)'
        elif isinstance(error, FileNotFoundError):
            reason += ' (' + str(error.filename) + '; run scripts/pair_t3.py or set SKYWATCH_T3_TOKEN_FILE)'
        elif isinstance(error, OSError):
            reason += ' (' + str(getattr(error, 'reason', None) or error.strerror or 'connection failed') + ')'
        print('Skywatch T3 bridge unavailable: ' + reason, file=sys.stderr)
        sys.exit(1)
