# The `event` and `nixevent` token grammar

Both commands take a space-separated token list:

```
event <format> <event-type>... [CUSTOM <subclass>...]
nixevent <event-type>... [CUSTOM <subclass>...]
```

`CUSTOM` is **terminal**. `parse_command` in `mod_event_socket.c` walks the
tokens with a sticky flag: until `CUSTOM` is seen each token is matched against
`switch_name_event()` and enables an event type; from `CUSTOM` onward every
token is inserted into the listener's subclass hash verbatim, and is never
matched against an event name again.

So this subscribes to two event types and one subclass:

```
event plain CHANNEL_CREATE HEARTBEAT CUSTOM sofia::register
```

and this subscribes to *no* channel events at all — `CHANNEL_CREATE` becomes a
subclass name that no event will ever carry:

```
event plain CUSTOM sofia::register CHANNEL_CREATE
```

Both return `+OK`. Nothing in the reply, the connection state, or the switch log
distinguishes them. Diagnosing the second form from the outside takes a second,
independently subscribed client receiving what the first one does not.

Event names are matched with `strcasecmp`, so `custom`, `Custom` and `CUSTOM`
are the same token.

## What this library guarantees

`EventSubscription` keeps event types and custom subclasses in separate fields,
so it always emits them in the correct positions. `order_event_tokens` performs
that ordering and is the only place it happens; `EventSubscription::to_event_string`
and `EslClient::nixevent` both route through it. The order a caller adds event
types in does not affect which ones get subscribed.

`EslClient::subscribe_events_raw` and `EslClient::nixevent_raw` send their string
verbatim and guarantee nothing. Once the list is one flat string the
event-type/subclass distinction is gone, so a check there would have to guess
which tokens were meant as which — and would guess wrong on subclass lists this
crate itself builds. Both return the one detectable case (a token after
`CUSTOM` that names a known event type) as a `#[must_use]` `SwallowedEventTypes`,
filled by `swallowed_event_types`.

## A bare `CUSTOM` subscribes to nothing

Enabling the `CUSTOM` event type is necessary but not sufficient. The dispatch
filter delivers a custom event only when its subclass name is found in the
listener's subclass hash, so `event plain CUSTOM` with no subclasses after it
receives no custom events at all — not all of them.

Only `ALL` fills that hash wholesale, via `set_all_custom()`, and it fills it
with the subclasses modules have reserved. An ad-hoc subclass created by a
`sendevent` from another connection is not among them.

## Related

`esl-allowed-events` in a directory user's parameters uses the same sticky-flag
parse, with the same consequence for ordering — see
[freeswitch-directory-users.md](freeswitch-directory-users.md).
