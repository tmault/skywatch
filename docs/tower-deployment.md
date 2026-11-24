# Deploying Skywatch on a homelab Docker host behind DockTail

This is the pattern used to run Skywatch as a managed container on a Linux homelab
host that also runs its own T3 server, published only to a Tailscale tailnet through
[DockTail](https://docktail.org/). Placeholders: `<host>` is the Docker host,
`<tailnet>` is the tailnet's MagicDNS suffix, `<t3-user>`/`<t3-uid>` own the host's
T3 server. Keep real names, tokens and IDs out of this repository.

## Shape

| Piece | Choice | Why |
| --- | --- | --- |
| Project | `/srv/compose/skywatch/compose.yaml` with `source/` = checkout of the deployed revision | One Compose project; the build context is the exact published commit |
| Image | `local/skywatch:<short-sha>`, built from `source/Dockerfile`, `pull_policy: never` | Each revision keeps its own tag for rollback; pull-based updaters skip it |
| Network | `network_mode: host`, `SKYWATCH_HOST=127.0.0.1`, `SKYWATCH_PORT=8422` | The bridge must reach T3 on host loopback; the unauthenticated server listens on loopback only |
| Publication | DockTail labels (`docktail.service.name: skywatch`, port `8422` http → `443` https, no `.network` label) | DockTail proxies host-network containers via `http://127.0.0.1:<port>`; access is gated by tailnet policy |
| User | `user: "<t3-uid>:<t3-gid>"`, build args `SKYWATCH_UID/GID` to match | T3's `userdata` directory is mode 0700 |
| T3 state | `<t3-home>/.t3/userdata` bind-mounted read-only; tmpfs over `secrets`, `relay-clients`, `provider-processes`, `opencode-servers`, `attachments`, `browser-artifacts`, `server-browser`, `logs` | Skywatch reads only `statev2.sqlite` (+ `-wal`/`-shm`) and `environment-id`; hide the rest |
| MCP token | Compose secret from `/srv/appdata/skywatch/t3-mcp-token` (owner `<t3-user>`, 0600, directory 0700) | Outside the image and repository |
| Hardening | `read_only`, `cap_drop: [ALL]`, `no-new-privileges`, `init`, `mem_limit`, bounded json-file logs, `restart: unless-stopped` | |

Skywatch shows the T3 server whose SQLite it mounts as the home airport, and that
server's *linked environments* as satellite airports. A host whose T3 has no peer links
shows only its own threads; link the owner's other devices to that T3 to see them.

## Pair the MCP bridge (once, on the host)

Run the pairing script as the T3 user. If that user cannot write the final secret
location, pair into a temporary file it owns and install it as root:

```bash
tmp=$(sudo -u <t3-user> mktemp -d <t3-home>/.skywatch-pair.XXXXXX)
sudo -u <t3-user> -H env SKYWATCH_T3_CLI=<path-to-t3-cli> SKYWATCH_T3_TOKEN_FILE="$tmp/t3-mcp-token" \
  python3 -I - < /srv/compose/skywatch/source/scripts/pair_t3.py
install -m 0600 -o <t3-user> -g <t3-user> "$tmp/t3-mcp-token" /srv/appdata/skywatch/t3-mcp-token
shred -u "$tmp/t3-mcp-token" && rmdir "$tmp"
```

The credential lasts 30 days (`lifetime seconds: 2592000`). Re-pair before it expires,
then `docker compose up -d --force-recreate`. Stale pairings can be listed with
`t3 auth session list` and removed with `t3 auth session revoke <id>`.

## Update and rollback

```bash
cd /srv/compose/skywatch
git -C source fetch origin && git -C source checkout --detach <new-sha>
sed -i "s/^SKYWATCH_TAG=.*/SKYWATCH_TAG=$(git -C source rev-parse --short HEAD)/" .env
docker compose build && docker compose up -d

# Rollback: point SKYWATCH_TAG at a retained image tag and recreate without building
docker image ls local/skywatch
sed -i "s/^SKYWATCH_TAG=.*/SKYWATCH_TAG=<previous-short-sha>/" .env
docker compose up -d --no-build
```

Skywatch keeps no server-side state, so rollback needs no data migration.

## Verify

```bash
docker inspect skywatch --format '{{.State.Health.Status}} {{.HostConfig.RestartPolicy.Name}}'
ss -tlnp | grep ':8422'                      # 127.0.0.1:8422 only
tailscale serve status --json | jq '.Services["svc:skywatch"]'   # Proxy http://127.0.0.1:8422
curl -fsS https://skywatch.<tailnet>/health   # from an authorized tailnet client
```

Check `/ws` from an authorized client: the first frame is a `snapshot`. From a tailnet
device outside the policy, the same request must fail, and the LAN address must not
answer on port 8422.
