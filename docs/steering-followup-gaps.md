# Steering & Follow-up: rab vs pi — Gap Analysis

## What's Aligned (after quick wins in c02f63c)

| Feature | pi | rab | Status |
|---|---|---|---|
| Queue on Enter while streaming → `steer()` | `agent.steer()` | `agent.steer()` | ✅ Identical |
| Queue on Alt+Enter while streaming → `followUp()` | `agent.followUp()` | `agent.follow_up()` | ✅ Identical |
| Steering drained mid-turn between tool batches | Inner loop polls `getSteeringMessages` | Same in yoagent `agent_loop.rs` | ✅ Identical |
| Follow-ups drained after inner loop exits | Outer loop polls `getFollowUpMessages` | Same in yoagent `agent_loop.rs` | ✅ Identical |
| Queue depth in status bar | Shows counts via `_emitQueueUpdate` | Shows counts via `steering_queue_len()` | ✅ Equivalent |
| No crash on agent-end race | N/A (single-threaded JS event loop) | Guarded by `agent.is_streaming()` check | ✅ Fixed |
| Idle follow-up with queued messages | Queues persist, drained on next prompt | Drained into `prompt_messages` on Alt+Enter | ✅ Equivalent |

## Remaining Gaps

### 1. Queue Management API (no UI to inspect/clear queued messages)

**pi**: `AgentSession` maintains parallel `_steeringMessages` / `_followUpMessages` string arrays.
Exposes:
- `getSteeringMessages(): readonly string[]`
- `getFollowUpMessages(): readonly string[]`
- `pendingMessageCount: number`
- `clearQueue(): { steering, followUp }` — returns drained messages for restoring to editor

These drive the queue display in the UI and let users un-queue mistaken messages.

**rab**: No equivalent. The agent's internal queues are opaque — `steering_queue_len()` only gives a count. No way to inspect content or clear selectively.

**Fix**: Add parallel `pending_steer_texts: Vec<String>` / `pending_follow_up_texts: Vec<String>` to `App`, updated whenever `agent.steer()`/`agent.follow_up()` is called. Add a `clear_queue()` method. Wire `Dequeue` action to show a queue overlay.

### 2. Extension-Origin steer/follow_up

**pi**: `sendUserMessage(content, { deliverAs: "steer" | "followUp" })` lets extensions queue messages with explicit delivery semantics.

**rab**: `submit_message()` infers steer vs follow-up from streaming state only. No `deliver_as` parameter exposed to extensions.

**Fix**: Add a `deliver_as` parameter to the extension message-sending path.

### 3. `assert!` panic instead of catchable error

**pi** (`agent.ts`):
```typescript
if (this.activeRun) {
    throw new Error("Agent is already processing. Use steer() or followUp()...");
}
```

**yoagent** (`agent.rs`):
```rust
assert!(
    !self.is_streaming,
    "Agent is already streaming. Use steer() or follow_up()."
);
```

Panics are unrecoverable in Rust. The race condition that triggered this is fixed (see Quick Win #2), but any future code path calling `prompt()` while streaming will crash hard.

**Fix**: Change yoagent's `agent.rs` to return `Result` or emit an `InputRejected` event through the channel instead of asserting. Requires touching yoagent.

### 4. `skipInitialSteeringPoll` (trivial)

**pi**: A `skipInitialSteeringPoll` flag prevents the loop from re-draining the steering queue when `continue()` already drained it before calling `runPromptMessages()`.

**rab/yoagent**: Always polls. When the queue is empty, the poll returns empty — harmless no-op.

**Fix**: Not worth the complexity. Zero impact.
