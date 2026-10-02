use std::borrow::Borrow;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::time::timeout;
use tracing::{debug, info};

use crate::{
    command::{EslCommand, EslResponse, ExecuteOptions},
    constants::{REPLY_COMMAND_NOT_FOUND, UNDEF_VALUE},
    error::{EslError, EslResult},
    event::{EslEvent, EslEventType, EventFormat},
    headers::EventHeader,
};

use super::{ConnectionStatus, EslClient, EslEventStream};

impl EslClient {
    /// Send a command and wait for the reply.
    ///
    /// The writer lock is held through the entire send-and-receive cycle to
    /// prevent concurrent commands from overwriting the pending reply slot
    /// (ESL is a sequential request/response protocol).
    pub async fn send_command(&self, command: EslCommand) -> EslResult<EslResponse> {
        if !self.is_connected() {
            return Err(EslError::NotConnected);
        }

        let command_str = command.to_wire_format()?;
        debug!(">> {}", command.redact_wire(&command_str));

        // Lock writer -- serializes concurrent commands and holds through reply.
        let mut writer = self
            .writer
            .lock()
            .await;

        // Set up reply channel
        let (tx, rx) = tokio::sync::oneshot::channel();
        {
            let mut pending = self
                .shared
                .pending_reply
                .lock()
                .await;
            // Checked under the same lock fail_pending_reply takes: the entry
            // is_connected() snapshot can go stale while awaiting the writer
            // lock, and a waiter installed after the reader exited would never
            // be woken.
            if pending.reader_dead {
                return Err(EslError::ConnectionClosed);
            }
            pending.waiting = Some(tx);
        }

        // Write command
        if let Err(e) = writer
            .write_all(command_str.as_bytes())
            .await
        {
            let mut pending = self
                .shared
                .pending_reply
                .lock()
                .await;
            // Mirror of the timeout branch, minus the stale_replies
            // increment: the command never hit the wire, so no reply can be
            // in flight for it. Leaving the sender installed would make
            // teardown_for_reexec see a phantom in-flight command.
            pending.waiting = None;
            drop(pending);
            drop(writer);
            return Err(EslError::Io(e));
        }

        // Wait for reply from reader task with command timeout (writer still locked)
        let timeout_ms = self
            .shared
            .command_timeout_ms
            .load(Ordering::Relaxed);
        let message = match timeout(Duration::from_millis(timeout_ms), rx).await {
            Ok(Ok(message)) => message,
            Ok(Err(e)) => {
                debug!("pending reply channel closed: {e}");
                drop(writer);
                return Err(EslError::ConnectionClosed);
            }
            Err(_) => {
                let mut pending = self
                    .shared
                    .pending_reply
                    .lock()
                    .await;
                // take() == Some: the reply is still in flight and would land on
                // the next waiter, so count it. None: the reader already took it.
                if pending
                    .waiting
                    .take()
                    .is_some()
                {
                    pending.stale_replies += 1;
                }
                drop(writer);
                return Err(EslError::Timeout { timeout_ms });
            }
        };

        drop(writer);

        let response = EslResponse::from_message(message);
        debug!("Received response: success={}", response.is_success());
        Ok(response)
    }

    /// Send a command and require a successful response, discarding the body.
    async fn send_command_ok(&self, command: EslCommand) -> EslResult<()> {
        self.send_command(command)
            .await?
            .into_result()
            .map(|_| ())
    }

    /// Execute API command synchronously.
    ///
    /// **Warning: this blocks the entire ESL socket.** FreeSWITCH processes
    /// `api` commands inline -- no events are delivered and no other commands
    /// can be sent on this connection until the command finishes. For commands
    /// that may take a long time (`originate`, `conference`, bulk operations),
    /// use [`bgapi`](Self::bgapi) instead so events keep flowing.
    ///
    /// [`EslResponse::api_result`] is the single check this needs. A command
    /// FreeSWITCH refuses (an `esl-allowed-api` gate) never runs, and is
    /// answered as a reply with no body; one that runs and fails reports in the
    /// body. `api_result` covers both. Use
    /// [`parse_api_body`](crate::parse_api_body) for `BACKGROUND_JOB` event
    /// bodies, which carry only the second kind.
    ///
    /// ```rust,no_run
    /// # async fn example(client: &freeswitch_esl_tokio::EslClient) -> Result<(), freeswitch_esl_tokio::EslError> {
    /// let resp = client.api("status").await?;
    /// let status = resp.api_result()?;
    /// println!("{}", status);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn api(&self, command: &str) -> EslResult<EslResponse> {
        let cmd = EslCommand::Api {
            command: command.to_string(),
        };
        self.send_command(cmd)
            .await
    }

    /// Execute background API command.
    ///
    /// Returns immediately with a `Job-UUID` in the response. The actual result
    /// arrives later as a [`EslEventType::BackgroundJob`] event -- subscribe to it
    /// and correlate via [`HeaderLookup::job_uuid()`](crate::HeaderLookup::job_uuid) / [`EslResponse::job_uuid`]:
    ///
    /// Correlating is a correctness requirement, not a way to find your result
    /// faster. `BACKGROUND_JOB` is fired on the global event bus, so every ESL
    /// client on the switch receives every other client's job results -- including
    /// an operator's `fs_cli -x 'bgapi ...'`. An event loop that acts on any
    /// `BACKGROUND_JOB` it sees will act on results it never asked for, and
    /// "I only issue one at a time" does not make the ones you see yours.
    /// [`BgJobTracker`](crate::BgJobTracker) owns the bookkeeping.
    ///
    /// A result is not guaranteed to arrive at all: it is delivered on the
    /// connection that issued the command, so a reconnect strands every job in
    /// flight. Pair correlation with a deadline of your own -- see
    /// [`BgJobTracker::retain`](crate::BgJobTracker::retain).
    ///
    /// ```rust,no_run
    /// # async fn example(client: &freeswitch_esl_tokio::EslClient) -> Result<(), freeswitch_esl_tokio::EslError> {
    /// let resp = client.bgapi("originate user/1000 &park").await?;
    /// let job_uuid = resp.job_uuid().expect("bgapi returns Job-UUID header");
    /// // Match against event.job_uuid() in the event loop
    /// # Ok(())
    /// # }
    /// ```
    pub async fn bgapi(&self, command: &str) -> EslResult<EslResponse> {
        let cmd = EslCommand::BgApi {
            command: command.to_string(),
        };
        self.send_command(cmd)
            .await
    }

    /// Subscribe to all events.
    ///
    /// Convenience wrapper for `subscribe_events(format, &[EslEventType::All])`.
    ///
    /// ```rust,no_run
    /// # async fn example(client: &freeswitch_esl_tokio::EslClient) -> Result<(), freeswitch_esl_tokio::EslError> {
    /// use freeswitch_esl_tokio::EventFormat;
    /// client.subscribe_all_events(EventFormat::Plain).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn subscribe_all_events(&self, format: EventFormat) -> EslResult<()> {
        self.subscribe_events(format, &[EslEventType::All])
            .await
    }

    /// Subscribe to events by typed enum variants.
    ///
    /// To subscribe to all events, use
    /// [`subscribe_all_events`](Self::subscribe_all_events).
    ///
    /// The order of `events` is not load-bearing: `CUSTOM` is emitted last
    /// whatever position it was given in.
    ///
    /// `EslEventType::Custom` on its own delivers **nothing**: FreeSWITCH gates
    /// custom events on the listener's subclass hash, and only `All` populates
    /// that wholesale. Name the subclasses you want on an
    /// [`EventSubscription`](freeswitch_types::EventSubscription) and pass it to
    /// [`apply_subscription`](Self::apply_subscription):
    ///
    /// ```rust,no_run
    /// # async fn example(client: &freeswitch_esl_tokio::EslClient) -> Result<(), Box<dyn std::error::Error>> {
    /// use freeswitch_esl_tokio::{EventFormat, EventSubscription};
    /// let sub = EventSubscription::new(EventFormat::Plain)
    ///     .custom_subclass("sofia::register")?
    ///     .custom_subclass("sofia::unregister")?;
    /// client.apply_subscription(&sub).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn subscribe_events<T: Borrow<EslEventType>>(
        &self,
        format: EventFormat,
        events: impl IntoIterator<Item = T>,
    ) -> EslResult<()> {
        let sub = freeswitch_types::EventSubscription::new(format).events(events);
        self.send_subscription_events(&sub)
            .await?;
        info!("Subscribed to events with format {:?}", format);
        Ok(())
    }

    /// Send the ESL `event` command for a subscription.
    ///
    /// Does nothing if the subscription has no events, raw events, or custom
    /// subclasses. Delegates ALL-collapse to [`EventSubscription::to_event_string`].
    async fn send_subscription_events(
        &self,
        sub: &freeswitch_types::EventSubscription,
    ) -> EslResult<()> {
        if let Some(events_str) = sub.to_event_string() {
            let cmd = EslCommand::Events {
                format: sub
                    .format()
                    .to_string(),
                events: events_str,
            };
            self.send_command_ok(cmd)
                .await?;
        }
        Ok(())
    }

    /// Send ESL `filter` commands for each (header, value) pair.
    async fn apply_filters(&self, filters: &[(String, String)]) -> EslResult<()> {
        for (header, value) in filters {
            self.filter_raw(header, value)
                .await?;
        }
        Ok(())
    }

    /// Send `nixevent` for strings present in `old` but absent from `new`.
    ///
    /// `prefix` is prepended to the joined event name list (use `"CUSTOM "` for
    /// custom subclasses, `""` for plain event names).
    async fn nixevent_str_diff<'a>(
        &self,
        old: &'a [String],
        new: &'a [String],
        prefix: &str,
    ) -> EslResult<()> {
        use std::collections::HashSet;
        let new_set: HashSet<&str> = new
            .iter()
            .map(|s| s.as_str())
            .collect();
        let removed: Vec<&str> = old
            .iter()
            .map(|s| s.as_str())
            .filter(|s| !new_set.contains(s))
            .collect();
        if !removed.is_empty() {
            let nixevent_str = format!("{}{}", prefix, removed.join(" "));
            self.nixevent_raw(&nixevent_str)
                .await?;
        }
        Ok(())
    }

    /// Subscribe to events using a raw event name string, sent verbatim.
    ///
    /// Prefer [`EventSubscription`](freeswitch_types::EventSubscription): its
    /// `custom_subclass()` covers `CUSTOM` subclasses and its `event_raw()`
    /// covers event types not yet in [`EslEventType`], and it orders the token
    /// list correctly. This method exists for a list neither can express.
    ///
    /// `CUSTOM` must come last: FreeSWITCH reads every token after it as a
    /// subclass name, so an event type placed after it is never subscribed and
    /// the command still returns `+OK`. Nothing here checks that — the
    /// event-type/subclass distinction is gone once the list is one flat
    /// string. [`swallowed_event_types`](freeswitch_types::event::swallowed_event_types)
    /// reports the detectable case.
    ///
    /// ```rust,no_run
    /// # async fn example(client: &freeswitch_esl_tokio::EslClient) -> Result<(), freeswitch_esl_tokio::EslError> {
    /// use freeswitch_esl_tokio::EventFormat;
    /// client.subscribe_events_raw(EventFormat::Plain, "HEARTBEAT CUSTOM sofia::register").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn subscribe_events_raw(&self, format: EventFormat, events: &str) -> EslResult<()> {
        let cmd = EslCommand::Events {
            format: format.to_string(),
            events: events.to_string(),
        };

        self.send_command_ok(cmd)
            .await?;
        info!(
            "Subscribed to raw events '{}' with format {:?}",
            events, format
        );
        Ok(())
    }

    /// Set event filter using a typed [`EventHeader`].
    pub async fn filter(&self, header: EventHeader, value: &str) -> EslResult<()> {
        self.filter_raw(header.as_str(), value)
            .await
    }

    /// Set event filter using a raw header name string.
    ///
    /// Prefer [`filter`](Self::filter) when the header has an [`EventHeader`]
    /// variant. Use this only for headers not yet covered by the typed enum.
    pub async fn filter_raw(&self, header: &str, value: &str) -> EslResult<()> {
        let cmd = EslCommand::Filter {
            header: header.to_string(),
            value: value.to_string(),
        };

        self.send_command_ok(cmd)
            .await?;
        debug!("Set event filter: {} = {}", header, value);
        Ok(())
    }

    /// Apply an [`EventSubscription`](freeswitch_types::EventSubscription) by sending its filters and event command.
    ///
    /// Sends each filter via [`filter_raw`](Self::filter_raw), then subscribes
    /// to the configured events. Does nothing if the subscription is empty.
    ///
    /// FreeSWITCH's `event` command is additive and idempotent -- subscribing
    /// to an event type that is already subscribed is a no-op (the server
    /// stores subscriptions as a boolean-per-type array). It is safe to call
    /// this on a connection that already has subscriptions; new types are
    /// added, existing ones are unaffected.
    ///
    /// The caller retains ownership of the subscription for use during
    /// reconnection.
    pub async fn apply_subscription(
        &self,
        sub: &freeswitch_types::EventSubscription,
    ) -> EslResult<()> {
        self.apply_filters(sub.filters())
            .await?;
        self.send_subscription_events(sub)
            .await
    }

    /// Replace the current subscription with a new one.
    ///
    /// The sequence is:
    ///
    /// 1. Send the new `event` command (additive -- no events are lost yet)
    /// 2. Clear all filters and re-add the new ones
    /// 3. `noevents` + re-send `event` to remove stale event types
    ///
    /// **Step 3 introduces a hard event-loss window.** `noevents` unsubscribes
    /// from all events; any events that arrive between `noevents` and the
    /// subsequent `event` command are permanently lost. Between steps 2 and 3
    /// there is also a brief window where stale event types from the old
    /// subscription may be delivered (extra events, not missing events).
    ///
    /// For loss-free subscription changes use
    /// [`resubscribe_from`](Self::resubscribe_from), which diffs old and new
    /// and uses `nixevent` to remove only the delta — new types are subscribed
    /// before old ones are removed, so no desired event type is ever
    /// unsubscribed.
    pub async fn resubscribe(&self, sub: &freeswitch_types::EventSubscription) -> EslResult<()> {
        // Step 1: add new event types (additive, no gap yet)
        self.send_subscription_events(sub)
            .await?;

        // Step 2: replace filters
        self.filter_delete_all()
            .await?;
        self.apply_filters(sub.filters())
            .await?;

        // Step 3: clear stale event types and re-apply (loss window: see doc)
        self.noevents()
            .await?;
        self.send_subscription_events(sub)
            .await
    }

    /// Replace the current subscription using a diff against the old one.
    ///
    /// Unlike [`resubscribe`](Self::resubscribe), this method produces
    /// no event gap at all. It computes the difference between `old` and
    /// `new` and applies only the minimal changes:
    ///
    /// 1. Subscribe to event types in `new` but not in `old` (additive)
    /// 2. Replace filters (clear all, re-add new)
    /// 3. `nixevent` event types in `old` but not in `new` (selective removal)
    /// 4. `nixevent` custom subclasses in `old` but not in `new`
    ///
    /// Because new subscriptions are applied before old ones are removed,
    /// there is never a moment where a desired event type is unsubscribed.
    ///
    /// The caller must provide the old subscription that was previously
    /// applied. If the old subscription doesn't match what the server
    /// actually has (e.g. after a reconnection), use [`resubscribe`](Self::resubscribe)
    /// instead.
    pub async fn resubscribe_from(
        &self,
        old: &freeswitch_types::EventSubscription,
        new: &freeswitch_types::EventSubscription,
    ) -> EslResult<()> {
        use std::collections::HashSet;

        let old_types: HashSet<EslEventType> = old
            .event_types()
            .iter()
            .copied()
            .collect();
        let new_types: HashSet<EslEventType> = new
            .event_types()
            .iter()
            .copied()
            .collect();

        // Step 1: subscribe to added event types + custom subclasses
        self.send_subscription_events(new)
            .await?;

        // Step 2: replace filters
        self.filter_delete_all()
            .await?;
        self.apply_filters(new.filters())
            .await?;

        // Step 3: nixevent removed typed event types
        let removed_types: Vec<EslEventType> = old_types
            .difference(&new_types)
            .copied()
            .collect();
        if !removed_types.is_empty() {
            self.nixevent(removed_types)
                .await?;
        }

        // Step 4: nixevent removed raw-named events
        self.nixevent_str_diff(old.event_types_raw(), new.event_types_raw(), "")
            .await?;

        // Step 5: nixevent removed custom subclasses
        self.nixevent_str_diff(
            old.custom_subclass_list(),
            new.custom_subclass_list(),
            "CUSTOM ",
        )
        .await
    }

    /// Execute application on channel.
    pub async fn execute(
        &self,
        app: &str,
        args: Option<&str>,
        uuid: Option<&str>,
    ) -> EslResult<EslResponse> {
        self.execute_with_options(app, args, uuid, ExecuteOptions::default())
            .await
    }

    /// Execute application on channel with custom options.
    ///
    /// See [`ExecuteOptions`] for available flags (`event-lock`, `async`, `loops`).
    pub async fn execute_with_options(
        &self,
        app: &str,
        args: Option<&str>,
        uuid: Option<&str>,
        options: ExecuteOptions,
    ) -> EslResult<EslResponse> {
        let cmd = EslCommand::Execute {
            app: app.to_string(),
            args: args.map(|s| s.to_string()),
            uuid: uuid.map(|s| s.to_string()),
            options,
        };
        self.send_command(cmd)
            .await
    }

    /// Send message to channel
    pub async fn sendmsg(&self, uuid: Option<&str>, event: EslEvent) -> EslResult<EslResponse> {
        let cmd = EslCommand::SendMsg {
            uuid: uuid.map(|s| s.to_string()),
            event,
        };
        self.send_command(cmd)
            .await
    }

    /// Fire an event into FreeSWITCH's event bus.
    ///
    /// Headers and body from the event are sent as-is (not percent-encoded).
    /// If the event has a `unique-id` header, FreeSWITCH also queues it
    /// directly to that session.
    pub async fn sendevent(&self, event: EslEvent) -> EslResult<EslResponse> {
        self.send_command(EslCommand::SendEvent { event })
            .await
    }

    /// Subscribe to session events (outbound mode, no UUID needed).
    ///
    /// Subscribes to all channel-related events for the attached session.
    /// In outbound mode, FreeSWITCH already knows the session UUID.
    pub async fn myevents(&self, format: EventFormat) -> EslResult<()> {
        let cmd = EslCommand::MyEvents {
            format: format.to_string(),
            uuid: None,
        };
        self.send_command_ok(cmd)
            .await
    }

    /// Subscribe to session events for a specific UUID (inbound mode).
    ///
    /// Subscribes to all channel-related events for the given session UUID.
    /// Use this in inbound mode where no session is attached to the socket.
    pub async fn myevents_uuid(&self, uuid: &str, format: EventFormat) -> EslResult<()> {
        let cmd = EslCommand::MyEvents {
            format: format.to_string(),
            uuid: Some(uuid.to_string()),
        };
        self.send_command_ok(cmd)
            .await
    }

    /// Keep the socket open after the channel hangs up (outbound mode).
    ///
    /// Without linger, the socket closes immediately on hangup. With linger,
    /// FreeSWITCH sends a `text/disconnect-notice` with `Content-Disposition: linger`
    /// and keeps the socket open so the client can drain remaining events.
    ///
    /// Pass `None` for indefinite linger, or `Some(Duration)` for a timeout.
    pub async fn linger(&self, timeout: Option<Duration>) -> EslResult<()> {
        self.send_command_ok(EslCommand::Linger { timeout })
            .await
    }

    /// Cancel linger mode (outbound mode).
    ///
    /// Only effective before the channel hangs up. After the disconnect notice
    /// has been sent, it's too late to cancel.
    pub async fn nolinger(&self) -> EslResult<()> {
        self.send_command_ok(EslCommand::NoLinger)
            .await
    }

    /// Resume dialplan execution when the socket disconnects (outbound mode).
    ///
    /// Without resume, the channel is hung up when the socket application exits.
    /// With resume, FreeSWITCH continues dialplan execution from where it left off.
    pub async fn resume(&self) -> EslResult<()> {
        self.send_command_ok(EslCommand::Resume)
            .await
    }

    /// Establish the outbound session by sending `connect` and receiving channel data.
    ///
    /// In outbound mode, this **must** be the first command sent after
    /// [`accept_outbound`](Self::accept_outbound). FreeSWITCH replies with a
    /// `command/reply` containing all channel variables for the session.
    ///
    /// The returned [`EslResponse`] contains the channel data as headers.
    /// Use [`EslResponse::header()`] to read individual channel variables
    /// (e.g., `Caller-Caller-ID-Number`, `Channel-Name`).
    pub async fn connect_session(&self) -> EslResult<EslResponse> {
        self.send_command(EslCommand::Connect)
            .await
    }

    /// Unsubscribe from specific events by typed enum variants.
    ///
    /// The inverse of [`subscribe_events`](Self::subscribe_events). Accepts
    /// multiple event types to unsubscribe from at once. As with subscribing,
    /// the order of `events` is not load-bearing.
    pub async fn nixevent<T: Borrow<EslEventType>>(
        &self,
        events: impl IntoIterator<Item = T>,
    ) -> EslResult<()> {
        let names: Vec<&str> = events
            .into_iter()
            .map(|e| {
                e.borrow()
                    .as_str()
            })
            .collect();
        let s = freeswitch_types::event::order_event_tokens(names, []).join(" ");
        self.nixevent_raw(&s)
            .await
    }

    /// Unsubscribe from events using a raw event name string, sent verbatim.
    ///
    /// Prefer [`nixevent`](Self::nixevent) when all event types have
    /// [`EslEventType`] variants. Use this only for `CUSTOM` subclasses or
    /// event types not yet covered by the typed enum.
    ///
    /// Carries the same terminal-`CUSTOM` rule and the same absence of a check
    /// as [`subscribe_events_raw`](Self::subscribe_events_raw).
    pub async fn nixevent_raw(&self, events: &str) -> EslResult<()> {
        let cmd = EslCommand::NixEvent {
            events: events.to_string(),
        };
        self.send_command_ok(cmd)
            .await
    }

    /// Unsubscribe from all events.
    ///
    /// Clears all event subscriptions. The server flushes any queued events.
    pub async fn noevents(&self) -> EslResult<()> {
        self.send_command_ok(EslCommand::NoEvents)
            .await
    }

    /// Remove an event filter for a typed [`EventHeader`].
    ///
    /// Without a value, removes all filters for the given header.
    /// With a value, removes only the filter matching that header+value pair.
    pub async fn filter_delete(&self, header: EventHeader, value: Option<&str>) -> EslResult<()> {
        self.filter_delete_raw(header.as_str(), value)
            .await
    }

    /// Remove an event filter using a raw header name string.
    ///
    /// Prefer [`filter_delete`](Self::filter_delete) when the header has an
    /// [`EventHeader`] variant. Use this only for headers not yet covered by
    /// the typed enum.
    ///
    /// Without a value, removes all filters for the given header.
    /// With a value, removes only the filter matching that header+value pair.
    pub async fn filter_delete_raw(&self, header: &str, value: Option<&str>) -> EslResult<()> {
        let cmd = EslCommand::FilterDelete {
            header: header.to_string(),
            value: value.map(|v| v.to_string()),
        };
        self.send_command_ok(cmd)
            .await
    }

    /// Remove all event filters.
    pub async fn filter_delete_all(&self) -> EslResult<()> {
        self.send_command_ok(EslCommand::FilterDeleteAll)
            .await
    }

    /// Redirect session events to the ESL connection (outbound mode).
    ///
    /// When `on` is true, events that would normally be processed internally
    /// by FreeSWITCH are instead sent to the ESL connection.
    pub async fn divert_events(&self, on: bool) -> EslResult<()> {
        self.send_command_ok(EslCommand::DivertEvents { on })
            .await
    }

    /// Read a channel variable (outbound mode).
    ///
    /// **Protocol quirk:** Unlike every other ESL command, `getvar` returns
    /// the raw variable value directly in `Reply-Text` with no `+OK`/`-ERR`
    /// prefix. This method hands back that text verbatim; do not use
    /// `into_result()` on the response -- it would misclassify the bare value as
    /// [`UnexpectedReply`](crate::EslError::UnexpectedReply).
    ///
    /// An unset variable produces an empty reply, which `mod_event_socket` then
    /// overwrites with its `-ERR`-prefixed filler for a reply nothing wrote to.
    /// It reads like a failure and is not one, so
    /// [`getvar_opt`](Self::getvar_opt) is the accessor to prefer.
    pub async fn getvar(&self, name: &str) -> EslResult<String> {
        let cmd = EslCommand::GetVar {
            name: name.to_string(),
        };
        let response = self
            .send_command(cmd)
            .await?;
        response
            .reply_text()
            .map(|s| s.to_string())
            .ok_or_else(|| EslError::protocol_error("getvar response missing Reply-Text header"))
    }

    /// Read a channel variable, distinguishing "unset" from a present value.
    ///
    /// A missing variable has three spellings and none of them is a value:
    /// the literal `_undef_`, an empty `Reply-Text`, and the filler
    /// `mod_event_socket` writes over a reply a command left empty. All three
    /// normalize to `Ok(None)`.
    ///
    /// That filler is spelled like an unrecognised command, and on this command
    /// it cannot be told apart from one -- `getvar` is only reachable on a
    /// session, so an inbound connection sending it gets the same answer as an
    /// outbound one reading an unset variable.
    pub async fn getvar_opt(&self, name: &str) -> EslResult<Option<String>> {
        let value = self
            .getvar(name)
            .await?;
        Ok((!getvar_is_unset(&value)).then_some(value))
    }

    /// Enable FreeSWITCH log forwarding at the given level.
    ///
    /// Log messages stream as events with `Content-Type: log/data`. The level
    /// is a [`LogLevel`](crate::LogLevel) or its wire spelling.
    ///
    /// ```rust,no_run
    /// # use freeswitch_esl_tokio::{EslClient, EslError, LogLevel};
    /// # async fn run(client: EslClient) -> Result<(), EslError> {
    /// client.log(LogLevel::Warning).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn log(&self, level: impl AsRef<str>) -> EslResult<EslResponse> {
        let cmd = EslCommand::Log {
            level: level
                .as_ref()
                .to_string(),
        };
        self.send_command(cmd)
            .await
    }

    /// Disable log forwarding.
    pub async fn nolog(&self) -> EslResult<EslResponse> {
        self.send_command(EslCommand::NoLog)
            .await
    }

    /// Send a no-op command (keepalive).
    pub async fn noop(&self) -> EslResult<EslResponse> {
        self.send_command(EslCommand::NoOp)
            .await
    }

    /// Send the `exit` command to gracefully close the ESL session.
    ///
    /// Unlike [`disconnect()`](Self::disconnect) which shuts down the TCP
    /// write half immediately, this sends the ESL `exit` command and waits
    /// for the server's reply before the connection closes.
    pub async fn exit(&self) -> EslResult<EslResponse> {
        self.send_command(EslCommand::Exit)
            .await
    }

    /// Whether this is an inbound (client→FreeSWITCH) or outbound
    /// (FreeSWITCH→client) connection.
    pub fn connection_mode(&self) -> super::ConnectionMode {
        self.shared
            .mode
    }

    /// Authentication response from the inbound connect handshake.
    ///
    /// For `userauth`, contains `Allowed-Events`, `Allowed-API`, and
    /// `Allowed-LOG` headers describing the user's access policy.
    /// Returns `None` for outbound connections (no auth handshake).
    pub fn auth_response(&self) -> Option<&EslResponse> {
        self.shared
            .auth_response
            .as_ref()
    }

    /// Number of events dropped due to a full event queue.
    pub fn dropped_event_count(&self) -> u64 {
        self.shared
            .dropped_event_count
            .load(Ordering::Relaxed)
    }

    /// Dispatches that had to wait for event-queue capacity.
    ///
    /// Zero under [`EventOverflow::DropIncoming`](super::EventOverflow).
    pub fn event_stall_count(&self) -> u64 {
        self.shared
            .event_stall_count
            .load(Ordering::Relaxed)
    }

    /// Time the reader has spent waiting for event-queue capacity.
    ///
    /// Read beside [`event_stall_count`](Self::event_stall_count): one long
    /// stall and many short ones are the same total and different problems.
    pub fn event_stall_duration(&self) -> Duration {
        Duration::from_nanos(
            self.shared
                .event_stall_nanos
                .load(Ordering::Relaxed),
        )
    }

    /// Set liveness timeout. Any inbound TCP traffic resets the timer.
    /// Set to zero to disable (default).
    ///
    /// The library never sends keepalives on its own -- the timer is fed only
    /// by traffic the server pushes. On a busy connection, ordinary event
    /// traffic keeps it alive. On an **idle** connection you must arrange the
    /// traffic yourself, typically by subscribing to [`EslEventType::Heartbeat`]
    /// (`subscribe_events(.., &[EslEventType::Heartbeat])`); FreeSWITCH then
    /// emits a `HEARTBEAT` every ~20s.
    ///
    /// If that subscription is **denied** -- a permission-restricted user
    /// (`esl-allowed-events` without `HEARTBEAT`) gets `-ERR permission denied`,
    /// detectable via [`EslError::is_permission_denied`] -- the subscribe call
    /// returns a recoverable error and the connection stays usable, but no
    /// heartbeats arrive. Do not enable this timeout for such a connection, or
    /// it will trip on idle while the socket is perfectly healthy. See
    /// `examples/reconnecting_client.rs` for the gated pattern.
    ///
    /// [`EslError::is_permission_denied`]: crate::EslError::is_permission_denied
    pub fn set_liveness_timeout(&self, duration: Duration) {
        self.shared
            .liveness_timeout_ms
            .store(duration.as_millis() as u64, Ordering::Relaxed);
    }

    /// Set command response timeout (default: 5 seconds).
    ///
    /// Applies to `send_command()`, `api()`, `bgapi()`, `subscribe_events()`,
    /// and all other methods that send a command and await a reply.
    ///
    /// If increased for long-running `api()` calls, also increase or disable
    /// the liveness timeout -- `api` blocks the socket, starving the liveness timer.
    ///
    /// Raise it on an outbound socket that drives applications. In static mode
    /// a `sendmsg` reply arrives only once the application has returned, so the
    /// default bounds a prompt rather than a round trip and any real
    /// `play_and_get_digits` outlives it. See
    /// [outbound-esl-quirks.md](https://github.com/voip-rs/freeswitch-esl-tokio/blob/master/docs/outbound-esl-quirks.md).
    pub fn set_command_timeout(&self, duration: Duration) {
        self.shared
            .command_timeout_ms
            .store(duration.as_millis() as u64, Ordering::Relaxed);
    }

    /// Whether the connection is alive (not yet disconnected).
    pub fn is_connected(&self) -> bool {
        matches!(
            *self
                .status_rx
                .borrow(),
            ConnectionStatus::Connected
        )
    }

    /// Current connection status snapshot.
    pub fn status(&self) -> ConnectionStatus {
        self.status_rx
            .borrow()
            .clone()
    }

    /// Disconnect from FreeSWITCH by shutting down the write half.
    ///
    /// Sets the connection status to [`crate::DisconnectReason::ClientRequested`]
    /// before closing the socket, so callers can distinguish client-initiated
    /// disconnects from server-initiated ones.
    pub async fn disconnect(&self) -> EslResult<()> {
        info!("Client requested disconnect");
        self.shared
            .status_tx
            .send_replace(ConnectionStatus::Disconnected(
                super::DisconnectReason::ClientRequested,
            ));
        let mut writer = self
            .writer
            .lock()
            .await;
        writer
            .shutdown()
            .await
            .map_err(EslError::Io)?;
        Ok(())
    }
}

impl EslEventStream {
    /// Receive the next event, or None if the channel is closed.
    ///
    /// Returns `Err(EslError::QueueFull)` if events were dropped because the
    /// application was not draining events fast enough. This is a one-time
    /// notification per overflow episode -- subsequent calls return real events.
    /// Parse errors from the reader task are also surfaced here.
    ///
    /// Matching `Some(Ok(event))` in a `while let` ends the loop on a parse
    /// error as well as on disconnect, with nothing to tell the two apart; use
    /// [`try_next`](Self::try_next) to propagate it instead.
    pub async fn recv(&mut self) -> Option<Result<EslEvent, EslError>> {
        self.rx
            .recv()
            .await
    }

    /// The `?`-friendly form of [`recv`](Self::recv): `Ok(None)` is end of
    /// stream, `Err` a reader fault that `recv` would have hidden behind the
    /// same loop exit.
    pub async fn try_next(&mut self) -> EslResult<Option<EslEvent>> {
        self.recv()
            .await
            .transpose()
    }

    /// Whether the connection is alive (not yet disconnected).
    pub fn is_connected(&self) -> bool {
        matches!(
            *self
                .status_rx
                .borrow(),
            ConnectionStatus::Connected
        )
    }

    /// Current connection status snapshot.
    pub fn status(&self) -> ConnectionStatus {
        self.status_rx
            .borrow()
            .clone()
    }
}

impl futures_util::Stream for EslEventStream {
    type Item = Result<EslEvent, EslError>;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        self.rx
            .poll_recv(cx)
    }
}

/// Whether a `getvar` reply means the variable is not set.
fn getvar_is_unset(reply: &str) -> bool {
    reply.is_empty() || reply == UNDEF_VALUE || reply == REPLY_COMMAND_NOT_FOUND
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn getvar_unset_covers_the_empty_reply_filler() {
        assert!(getvar_is_unset(""));
        assert!(getvar_is_unset(UNDEF_VALUE));
        assert!(getvar_is_unset(REPLY_COMMAND_NOT_FOUND));
        assert!(!getvar_is_unset("sofia/internal/1000@example.com"));
    }
}
