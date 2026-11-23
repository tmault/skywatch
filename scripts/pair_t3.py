import base64,hashlib,json,secrets,subprocess,urllib.request,urllib.parse
from pathlib import Path
home=Path.home()/'.t3/userdata'
origin=json.loads((home/'server-runtime.json').read_text())['origin'].rstrip('/')
admin=subprocess.check_output(['t3','auth','session','issue','--scope','access:write','--scope','orchestration:read','--scope','orchestration:operate','--ttl','5m','--label','Skywatch setup','--token-only'],stderr=subprocess.DEVNULL).decode().strip()
def post(path, data, form=False, auth=False):
 body=urllib.parse.urlencode(data).encode() if form else json.dumps(data).encode()
 headers={'Content-Type':'application/x-www-form-urlencoded' if form else 'application/json'}
 if auth: headers['Authorization']='Bearer '+admin
 with urllib.request.urlopen(urllib.request.Request(origin+path,body,headers),timeout=10) as r:return json.load(r)
redirect='http://127.0.0.1:18423/callback'
client=post('/oauth/mcp/register',{'client_name':'Skywatch','redirect_uris':[redirect]})
verifier=secrets.token_urlsafe(48)
challenge=base64.urlsafe_b64encode(hashlib.sha256(verifier.encode()).digest()).decode().rstrip('=')
authorization={'response_type':'code','client_id':client['client_id'],'redirect_uri':redirect,'code_challenge':challenge,'code_challenge_method':'S256','resource':origin+'/mcp','state':secrets.token_urlsafe(24)}
approval=post('/oauth/mcp/approval',authorization,auth=True)
result=post('/oauth/mcp/decision',{'authorization':authorization,'decision':{'_tag':'browser-session','access':'approval-required','csrfToken':approval['csrfToken']}},auth=True)
code=urllib.parse.parse_qs(urllib.parse.urlparse(result['redirectTo']).query)['code'][0]
result=post('/oauth/mcp/token',{'grant_type':'authorization_code','client_id':client['client_id'],'redirect_uri':redirect,'code':code,'code_verifier':verifier,'resource':origin+'/mcp'},form=True)
p=Path.home()/'.config/skywatch/t3-mcp-token';p.parent.mkdir(parents=True,exist_ok=True,mode=0o700);p.touch(mode=0o600);p.write_text(result['access_token']);p.chmod(0o600)
print('Paired Skywatch MCP client; credential saved privately. Lifetime seconds:',result['expires_in'])
