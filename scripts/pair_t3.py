"""Pair a dedicated, approval-required Skywatch MCP client with the local T3 server.

Run once on the T3 host (Linux, macOS, or Windows):  python scripts/pair_t3.py
"""
import base64, hashlib, json, os, secrets, shutil, subprocess, sys, urllib.parse, urllib.request
from pathlib import Path

home = Path(os.environ.get('T3CODE_HOME') or Path.home() / '.t3')
origin = json.loads((home / 'userdata' / 'server-runtime.json').read_text(encoding='utf-8'))['origin'].rstrip('/')
# Same rules as the bridge: wildcard binds become loopback, and loopback skips proxies.
parts = urllib.parse.urlsplit(origin)
wildcard = {'0.0.0.0': '127.0.0.1', '::': '[::1]'}
if parts.hostname in wildcard:
    origin = urllib.parse.urlunsplit(parts._replace(netloc=wildcard[parts.hostname] + (':' + str(parts.port) if parts.port else '')))
loopback = urllib.parse.urlsplit(origin).hostname in {'127.0.0.1', 'localhost', '::1'}
opener = urllib.request.build_opener(*([urllib.request.ProxyHandler({})] if loopback else []))

# shutil.which finds t3.cmd on Windows; T3 Code also installs a launcher under <T3 home>/bin.
t3 = os.environ.get('SKYWATCH_T3_CLI') or shutil.which('t3') or shutil.which('t3', path=str(home / 'bin'))
if not t3:
    sys.exit('t3 CLI not found on PATH or in ' + str(home / 'bin') + '; set SKYWATCH_T3_CLI to its full path.')
admin = subprocess.check_output([t3, 'auth', 'session', 'issue', '--scope', 'access:write', '--scope', 'orchestration:read',
                                 '--scope', 'orchestration:operate', '--ttl', '5m', '--label', 'Skywatch setup', '--token-only'],
                                stderr=subprocess.DEVNULL).decode().strip()


def post(path, data, form=False, auth=False):
    body = urllib.parse.urlencode(data).encode() if form else json.dumps(data).encode()
    headers = {'Content-Type': 'application/x-www-form-urlencoded' if form else 'application/json'}
    if auth:
        headers['Authorization'] = 'Bearer ' + admin
    with opener.open(urllib.request.Request(origin + path, body, headers), timeout=10) as r:
        return json.load(r)


redirect = 'http://127.0.0.1:18423/callback'
client = post('/oauth/mcp/register', {'client_name': 'Skywatch', 'redirect_uris': [redirect]})
verifier = secrets.token_urlsafe(48)
challenge = base64.urlsafe_b64encode(hashlib.sha256(verifier.encode()).digest()).decode().rstrip('=')
authorization = {'response_type': 'code', 'client_id': client['client_id'], 'redirect_uri': redirect, 'code_challenge': challenge,
                 'code_challenge_method': 'S256', 'resource': origin + '/mcp', 'state': secrets.token_urlsafe(24)}
approval = post('/oauth/mcp/approval', authorization, auth=True)
result = post('/oauth/mcp/decision', {'authorization': authorization, 'decision': {
    '_tag': 'browser-session', 'access': 'approval-required', 'csrfToken': approval['csrfToken']}}, auth=True)
code = urllib.parse.parse_qs(urllib.parse.urlparse(result['redirectTo']).query)['code'][0]
result = post('/oauth/mcp/token', {'grant_type': 'authorization_code', 'client_id': client['client_id'], 'redirect_uri': redirect,
                                   'code': code, 'code_verifier': verifier, 'resource': origin + '/mcp'}, form=True)
# On Windows the mode bits are ignored; the file inherits the user profile's ACL.
p = Path(os.environ.get('SKYWATCH_T3_TOKEN_FILE') or Path.home() / '.config' / 'skywatch' / 't3-mcp-token')
p.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
p.touch(mode=0o600)
p.write_text(result['access_token'], encoding='utf-8')
p.chmod(0o600)
print('Paired Skywatch MCP client; credential saved privately to', p, '- lifetime seconds:', result['expires_in'])
