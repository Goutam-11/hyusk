-- Hyusk Indicator — a Noctalia v5 bar widget for the Hyusk desktop agent.
--
-- The agent already publishes everything here in $XDG_RUNTIME_DIR:
--   hyusk-status.json  state, active model, latest card, awaiting_reply, timer
--   hyusk-control.json {revision, action, ...} that the agent polls every 250 ms
-- Both are the GNOME indicator's contract, so this widget is a reader plus one
-- small file write; nothing in the agent is Noctalia-specific.
--
-- On the bar it shows what the GNOME indicator's panel button shows: the state
-- glyph, the state name beside it, and a running timer's countdown. Everything
-- else the GNOME menu has (transport, approvals, workflows, phone pairing, models
-- and API keys) lives in the panel behind the gear, in settings.luau.

-- Tabler glyph names Noctalia renders on the bar, one per HyuskState.
local STATE_GLYPHS = {
  hidden = "butterfly",
  waking = "ear",
  listening = "microphone",
  thinking = "brain",
  working = "tool",
  speaking = "volume",
}

-- Palette role per state: quiet when idle, accented while working.
local STATE_ROLES = {
  hidden = "secondary",
  waking = "primary",
  listening = "primary",
  thinking = "primary",
  working = "primary",
  speaking = "secondary",
}

-- Short state names, spelled as the GNOME indicator's panel label spells them.
-- The resting "idle" one stays off the bar: the calm glyph already says it.
local STATE_LABELS = {
  hidden = "idle",
  waking = "waking",
  listening = "listening",
  thinking = "thinking",
  working = "working",
  speaking = "speaking",
}

-- Used only when the palette has no such role, so the widget never renders
-- with an empty colour string.
local FALLBACK = { secondary = "#9AA0A6", primary = "#7DD3FC", error = "#F87171" }

local function runtime_dir()
  return noctalia.getenv("XDG_RUNTIME_DIR") or "/tmp"
end

local STATUS = runtime_dir() .. "/hyusk-status.json"
local CONTROL = runtime_dir() .. "/hyusk-control.json"
local PANEL_ID = "hyusk/indicator:settings"
-- The agent rewrites the status file on every state change; once a second is
-- enough to follow a turn, and it is also the tick a timer countdown needs.
local INTERVAL_MS = 1000

local function decode(raw)
  if type(raw) ~= "string" or raw == "" then
    return nil
  end
  local ok, value = pcall(noctalia.json.decode, raw)
  if ok and type(value) == "table" then
    return value
  end
  return nil
end

local function color(role)
  local hex = noctalia.getColor and noctalia.getColor(role)
  return hex or FALLBACK[role] or FALLBACK.secondary
end

local function shorten(text, limit)
  if type(text) ~= "string" then
    return nil
  end
  text = text:gsub("%s+", " ")
  if #text > limit then
    text = text:sub(1, limit - 1) .. "…"
  end
  if text == "" then
    return nil
  end
  return text
end

local function now_ms()
  return (noctalia.nowMs and noctalia.nowMs()) or 0
end

-- "12:34", or "1:02:03" once a timer runs past the hour.
local function format_seconds(total)
  total = math.max(0, math.floor(tonumber(total) or 0))
  local hours = math.floor(total / 3600)
  local minutes = math.floor((total % 3600) / 60)
  local seconds = total % 60
  if hours > 0 then
    return string.format("%d:%02d:%02d", hours, minutes, seconds)
  end
  return string.format("%d:%02d", minutes, seconds)
end

local function state_of(status)
  return type(status.state) == "string" and status.state:lower() or "hidden"
end

-- The card kind is the agent's own tag ("approval", "error", "response", …), so
-- a confirmation waiting for a yes/no is surfaced without guessing from text.
local function is_approval(status)
  local card = status.latest
  return type(card) == "table" and card.kind == "approval"
end

-- A timer the agent is running, as a live countdown, plus what it is for. The
-- agent clears `timer` when it fires, so anything here is still counting.
local function timer_text(status)
  local timer = status.timer
  if type(timer) ~= "table" or type(timer.ends_at_ms) ~= "number" then
    return nil
  end
  local remaining = math.max(0, math.ceil((timer.ends_at_ms - now_ms()) / 1000))
  return format_seconds(remaining), shorten(timer.label, 40)
end

local function busy_state(state)
  return state == "listening" or state == "thinking" or state == "working" or state == "speaking"
end

-- Key/value rows give the pointer a compact readout: what the agent is doing,
-- what it is doing it with, and the reply or timer it is waiting on.
local function tooltip(status, state, countdown, timer_label)
  local rows = { { "Hyusk", STATE_LABELS[state] or state } }

  local provider = status.active_provider
  local model = status.active_model
  if type(model) == "string" and model ~= "" then
    rows[#rows + 1] = {
      type(provider) == "string" and provider ~= "" and provider or "Model",
      model,
    }
  end

  if countdown then
    rows[#rows + 1] = { "Timer", timer_label and (countdown .. " · " .. timer_label) or countdown }
  end

  local card = status.latest
  if type(card) == "table" then
    local text = shorten(card.text, 400)
    if text then
      rows[#rows + 1] = { card.kind == "approval" and "Needs your answer" or "Latest", text }
    end
  end

  if status.awaiting_reply == true then
    rows[#rows + 1] = { "Microphone", "Waiting for your reply" }
  end

  rows[#rows + 1] = { "Click", busy_state(state) and "stop the agent" or "start listening" }

  return rows
end

local painted = false
-- Last painted state, so a click can mean "talk" or "stop" depending on what
-- the agent is doing right now.
local last_state = "hidden"

-- A vertical bar clips the capsule to the bar's thickness, so side bars get a
-- column and horizontal ones a row.
local function layout(controls)
  if barWidget.isVertical and barWidget.isVertical() then
    return ui.column({ gap = 4, align = "center" }, controls)
  end
  return ui.row({ gap = 6, align = "center" }, controls)
end

-- The gear is the way into model switching and API keys on a bar-only desktop;
-- the glyph itself keeps the widget's own left-click meaning (talk or stop).
local function paint(glyph, role, tip, label, countdown)
  local controls = { ui.glyph({ name = glyph, color = color(role) }) }
  if label then
    controls[#controls + 1] = ui.label({
      text = label,
      fontSize = 12,
      maxLines = 1,
      color = color(role),
    })
  end
  if countdown then
    controls[#controls + 1] = ui.glyph({
      name = "hourglass",
      size = 14,
      color = color(role),
    })
    controls[#controls + 1] = ui.label({
      text = countdown,
      fontSize = 12,
      fontWeight = "semibold",
      maxLines = 1,
      color = color(role),
    })
  end
  controls[#controls + 1] = ui.button({
    glyph = "adjustments",
    variant = "ghost",
    tooltip = "Hyusk models & API keys",
    onClick = function()
      noctalia.togglePanel(PANEL_ID)
    end,
  })

  barWidget.render(layout(controls))
  barWidget.setTooltip(tip)
end

local function render()
  local status = decode((noctalia.readFile(STATUS)))
  if not status then
    -- The agent writes this file in place, so a read can land mid-write, and
    -- an agent that is restarting has no file at all. Keep the last good paint
    -- rather than blanking the bar on every transient.
    if not painted then
      paint(STATE_GLYPHS.hidden, "secondary", "Hyusk is not running", nil, nil)
    end
    return
  end

  local state = state_of(status)
  last_state = state
  local approval = is_approval(status)
  local countdown, timer_label = timer_text(status)
  local label = approval and "confirm" or (state ~= "hidden" and STATE_LABELS[state] or nil)
  local glyph = approval and "shield-question" or (STATE_GLYPHS[state] or STATE_GLYPHS.hidden)
  local role = approval and "error" or (STATE_ROLES[state] or STATE_ROLES.hidden)

  paint(glyph, role, tooltip(status, state, countdown, timer_label), label, countdown)
  painted = true
end

-- The agent ignores any control whose revision is not newer than the last one
-- it saw, so each action must advance it. nowMs keeps that monotonic across
-- restarts where the previous write is still on disk.
local function next_revision()
  local revision = 0
  local existing = decode((noctalia.readFile(CONTROL)))
  if existing and type(existing.revision) == "number" then
    revision = existing.revision
  end
  return math.max(now_ms(), revision + 1)
end

local function send(action)
  local payload = string.format('{"revision":%d,"action":"%s"}', next_revision(), action)
  local ok, err = noctalia.writeFile(CONTROL, payload)
  if not ok then
    noctalia.notifyError("Hyusk", err or "could not write " .. CONTROL)
  end
  return ok
end

-- One button, two meanings: while the agent is idle it starts a capture exactly
-- as the wake word does; while it is thinking or talking the same click stops it.
-- Without this there is no way to silence a long reply on a bar-only desktop.
local function busy()
  return busy_state(last_state)
end

function onClick()
  if busy() then
    send("stop_task")
  else
    send("start_listening")
  end
end

function onRightClick()
  send("dismiss_card")
end

-- Lets `noctalia msg plugin hyusk/indicator:hyusk all stop_task` (a keybind)
-- drive the widget, and gives the install check something to call.
function onIpc(event)
  if event == "start_listening" or event == "stop_task" or event == "dismiss_card" then
    send(event)
  end
end

function update()
  noctalia.setUpdateInterval(INTERVAL_MS)
  render()
end

render()
noctalia.setUpdateInterval(INTERVAL_MS)
