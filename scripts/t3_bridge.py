"""Scoped MCP transport for Skywatch. JSON on stdin/stdout; never logs credentials."""
import json
import os
from pathlib import Path
import sys
import urllib.request


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
            runtime = json.loads((home / 'userdata/server-runtime.json').read_text())
            self.endpoint = runtime['origin'].rstrip('/') + '/mcp'
        token_file = os.environ.get('SKYWATCH_T3_TOKEN_FILE', str(Path.home() / '.config/skywatch/t3-mcp-token'))
        if not token_file:
            raise ValueError('SKYWATCH_T3_TOKEN_FILE must name a paired MCP client credential')
        self.token = Path(token_file).read_text().strip()
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
        with urllib.request.urlopen(request, timeout=8) as response:
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


def list_peer_threads(client, environment_id):
    projects = {}
    try:
        cursor = None
        while True:
            args = {'environmentId': environment_id, 'limit': 100}
            if cursor is not None:
                args['cursor'] = cursor
            page = client.call('t3_project_list', args)
            for proj in page.get('projects', []):
                title = (proj.get('title') or '').strip()
                if proj.get('id') and title:
                    projects[proj['id']] = title
            cursor = page.get('nextCursor')
            if cursor is None:
                break
    except Exception:
        pass

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
    request = json.load(sys.stdin)
    client = Client()
    if request.get('action') == 'send':
        return client.call('t3_thread_send', request['arguments'])
    peers = client.call('t3_environment_links', {}).get('links', [])
    result = []
    for peer in peers:
        item = {'environment_id': peer['environmentId'], 'label': peer['label'],
                'online': peer['status'] == 'reachable', 'threads': None}
        if item['online']:
            try:
                item['threads'] = list_peer_threads(client, peer['environmentId'])
            except Exception:
                item['online'] = False
        result.append(item)
    return result


if __name__ == '__main__':
    try:
        print(json.dumps(main()))
    except Exception as error:
        print('Skywatch T3 bridge unavailable: ' + type(error).__name__, file=sys.stderr)
        sys.exit(1)
