# Built-in realtime protocol

The application provides a dependency-free realtime layer over one authenticated
WebSocket connection per browser tab or client.

## Browser client

The typed, dependency-free client is built from `frontend/src/realtime.ts` and
served by the application, so there is nothing to install:

```html
<script type="module">
  import { RealtimeClient } from "/assets/app/realtime.js"

  const realtime = new RealtimeClient()
  realtime.subscribe("general")

  realtime.on("general:file.ready", ({ payload }) => {
    console.log(payload)
  })
</script>
```

The browser's existing session cookie authenticates the `/ws` connection. The
client reconnects automatically, uses application-level heartbeats, and restores
all subscriptions after reconnecting. Browser WebSocket requests must have an
`Origin` matching the request `Host`.

## Client commands

Commands sent to `/ws` are JSON objects:

```json
{"action":"subscribe","channel":"general"}
{"action":"unsubscribe","channel":"general"}
{"action":"ping","id":"optional-request-id"}
```

Server messages have a stable event envelope:

```json
{
  "event": "file.ready",
  "channel": "general",
  "payload": {"file_id": 42}
}
```

Protocol events currently include:

- `connection.established`
- `subscription.succeeded`
- `subscription.removed`
- `presence.member_added`
- `presence.member_removed`
- `pong`
- `error`

Private application events are delivered to every connected tab belonging to
the affected user; they do not require a channel subscription. Uploads emit:

- `upload.queued`
- `upload.completed`
- `upload.failed`

Drive mutations emit user-private invalidation events:

- `file.created`
- `file.moved`
- `file.deleted`
- `folder.created`
- `folder.renamed`
- `folder.moved`
- `folder.deleted`

The frontend treats HTTP APIs as authoritative. It debounces refreshes after
drive events and reconciles active upload jobs over HTTP after reconnecting or
when a terminal upload event does not arrive. This means events may safely be
dropped for a slow connection without blocking application requests.

Application event definitions live in `src/events`. Transport-level event names
live in `src/events/protocol.rs`.

Presence subscriptions return the current unique user IDs in `payload.members`.
Multiple connections belonging to one user count as one presence member.

## Declaring channels

Channels are registered in `src/routes/channels_routes.rs`:

```rust
Channel::new("general", |_user_id| true);
Channel::private("private", |user_id| user_id == 1);
Channel::presence("presence", |_user_id| true);
```

The callback runs whenever an authenticated user tries to subscribe.
It is also enforced when an authenticated client publishes through the HTTP
event endpoint.

## Publishing from the backend

Trusted backend code can publish an event with the configured `API_TOKEN`:

```sh
curl -X POST http://localhost:8080/api/realtime/events \
  -H 'Authorization: Bearer YOUR_EXISTING_API_TOKEN' \
  -H 'Content-Type: application/json' \
  -d '{
    "channel": "general",
    "event": "file.ready",
    "payload": {"file_id": 42}
  }'
```

The response reports how many subscribed socket connections accepted the event:

```json
{"delivered":2}
```

Session cookies and session bearer tokens cannot use the publishing endpoint.
They may only connect and subscribe, which prevents browser clients from forging
server-owned events.

## Deployment limits

The hub is intentionally in-memory and lightweight. A single server process is
fully supported. Running multiple application instances requires a shared
pub/sub adapter (for example Redis or NATS) so events reach sockets connected to
other instances.
