-- Hyusk Indicator — a Noctalia v5 bar widget for the Hyusk desktop agent.
--
-- The agent already publishes everything here in $XDG_RUNTIME_DIR:
--   hyusk-status.json  state, active model, latest card, awaiting_reply
--   hyusk-control.json {revision, action, ...} that the agent polls every 250 ms
-- Both are the GNOME indicator's contract, so this widget is a reader plus one
-- small file write; nothing in the agent is Noctalia-specific.

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
-- enough to follow a turn without waking the shell for nothing.
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

-- The card kind is the agent's own tag ("approval", "error", "response", …), so
-- a confirmation waiting for a yes/no is surfaced without guessing from text.
local function is_approval(status)
  local card = status.latest
  return type(card) == "table" and card.kind == "approval"
end

local function tooltip(status)
  local state = type(status.state) == "string" and status.state:lower() or "hidden"
  local lines = { "Hyusk — " .. state }

  local provider = status.active_provider
  local model = status.active_model
  if type(model) == "string" and model ~= "" then
    lines[#lines + 1] = (type(provider) == "string" and provider ~= "" and (provider .. " · ") or "") .. model
  end

  local card = status.latest
  if type(card) == "table" then
    local text = shorten(card.text, 400)
    if text then
      lines[#lines + 1] = (card.kind == "approval" and "Needs your answer — " or "") .. text
    end
  end

  if status.awaiting_reply == true then
    lines[#lines + 1] = "Waiting for your reply"
  end

  local busy = state == "listening" or state == "thinking" or state == "working" or state == "speaking"
  lines[#lines + 1] = busy and "Click to stop" or "Click to talk"

  return table.concat(lines, "\n"), state
end

local painted = false
-- Last painted state, so a click can mean "talk" or "stop" depending on what
-- the agent is doing right now.
local last_state = "hidden"

-- The gear is the way into model switching and API keys on a bar-only desktop;
-- the glyph itself keeps the widget's own left-click meaning (talk or stop).
local function paint(glyph, role, tip)
  barWidget.render(ui.row({ gap = 6, align = "center" }, {
    ui.glyph({ name = glyph, color = color(role) }),
    ui.button({
      glyph = "adjustments",
      variant = "ghost",
      tooltip = "Hyusk models & API keys",
      onClick = function()
        noctalia.togglePanel(PANEL_ID)
      end,
    }),
  }))
  barWidget.setTooltip(tip)
end

local function render()
  local status = decode(noctalia.readFile(STATUS))
  if not status then
    -- The agent writes this file in place, so a read can land mid-write, and
    -- an agent that is restarting has no file at all. Keep the last good paint
    -- rather than blanking the bar on every transient.
    if not painted then
      paint(STATE_GLYPHS.hidden, "secondary", "Hyusk is not running")
    end
    return
  end

  local tip, state = tooltip(status)
  last_state = state
  local approval = is_approval(status)
  local glyph = approval and "shield-question" or (STATE_GLYPHS[state] or STATE_GLYPHS.hidden)
  local role = approval and "error" or (STATE_ROLES[state] or STATE_ROLES.hidden)

  paint(glyph, role, tip)
  painted = true
end

-- The agent ignores any control whose revision is not newer than the last one
-- it saw, so each action must advance it. nowMs keeps that monotonic across
-- restarts where the previous write is still on disk.
local function next_revision()
  local revision = 0
  local existing = decode(noctalia.readFile(CONTROL))
  if existing and type(existing.revision) == "number" then
    revision = existing.revision
  end
  local now = (noctalia.nowMs and noctalia.nowMs()) or 0
  return math.max(now, revision + 1)
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
  return last_state == "listening"
    or last_state == "thinking"
    or last_state == "working"
    or last_state == "speaking"
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
