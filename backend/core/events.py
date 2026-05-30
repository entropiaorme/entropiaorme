"""Event type constants for the event bus."""

# ── Chat.log parser events ──

EVENT_COMBAT = "combat"
EVENT_LOOT_GROUP = "loot_group"
EVENT_SKILL_GAIN = "skill_gain"
EVENT_ENHANCER_BREAK = "enhancer_break"
EVENT_GLOBAL = "global"

# ── Hotbar / tool events ──

EVENT_ACTIVE_TOOL_CHANGED = "active_tool_changed"
EVENT_ACTIVE_HEAL_TOOL_CHANGED = "active_heal_tool_changed"

# ── Tracking session events ──

EVENT_SESSION_STARTED = "session_started"
EVENT_SESSION_STOPPED = "session_stopped"

# ── Mission events ──

EVENT_MISSION_RECEIVED = "mission_received"

# ── Tick boundary ──

# Published by the chatlog watcher at the end of each settled parse tick, after
# every per-event subscriber write for that tick has completed. Lets a stateful
# subscriber (the tracker) coalesce a tick's worth of low-level mutations into a
# single coarse domain event rather than emitting one per raw mutation. Purely
# intra-backend, like the other EVENT_* constants.
EVENT_TICK_FLUSHED = "tick_flushed"
