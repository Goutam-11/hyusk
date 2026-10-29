-- Hyusk settings panel — the [[panel]] entry behind the bar widget's gear.
--
-- The agent stays the only writer of its own configuration: this panel reads
-- $XDG_RUNTIME_DIR/hyusk-status.json (state, active model, catalogs, latest card,
-- timer) and writes requests to hyusk-control.json, exactly as the GNOME
-- indicator does. A pasted API key is handed to the agent, which stores it in the
-- login keyring and immediately overwrites the control file so the key does not
-- linger there.
--
-- The tabs carry the rest of the GNOME indicator's menu: Agent is the transport
-- keys, the response card (approve, deny, copy, more, dismiss) and a running
-- timer's countdown; Models is the provider picker and its catalog; Workflows
-- runs a workflow by name or opens the editor app; Device pairs a phone and opens
-- the automation browser. The API key form stays pinned under the tabs, so a key
-- can be saved from any tab.

local function runtime_dir()
  return noctalia.getenv("XDG_RUNTIME_DIR") or "/tmp"
end

local STATUS = runtime_dir() .. "/hyusk-status.json"
local CONTROL = runtime_dir() .. "/hyusk-control.json"
local PAIRING = runtime_dir() .. "/hyusk-pairing.json"
-- This plugin's own directory, where install-noctalia-plugin.sh puts the two
-- companion scripts the Device tab runs.
local PLUGIN_DIR = (noctalia.pluginDir and noctalia.pluginDir()) or ""

-- Providers whose credential is an HTTP API key the keyring can hold. Nova Sonic
-- (AWS profile) and Codex (CLI login) are select-only.
local KEY_PROVIDERS = {
  { id = "openrouter", label = "OpenRouter" },
  { id = "openai", label = "OpenAI" },
  { id = "bedrock", label = "Amazon Bedrock" },
}

local PROVIDER_LABELS = {
  openrouter = "OpenRouter",
  openai = "OpenAI",
  bedrock = "Amazon Bedrock",
  ["bedrock-sonic"] = "Nova Sonic",
  codex = "Codex",
}

-- Sentence-case state names, as the GNOME indicator's card and menu header
-- spell them.
local STATE_STATUS = {
  hidden = "Ready",
  waking = "Waking up",
  listening = "Listening",
  thinking = "Thinking",
  working = "Working",
  speaking = "Speaking",
}

-- The tabs, in the order the GNOME menu presents the same controls.
local TABS = {
  { id = "agent", label = "Agent" },
  { id = "models", label = "Models" },
  { id = "workflows", label = "Workflows" },
  { id = "device", label = "Device" },
}

-- Ceiling on rendered rows: the lists scroll, but a catalog of several hundred
-- models should not build several hundred controls on every render.
local MAX_MODEL_ROWS = 200
local MAX_WORKFLOW_ROWS = 100
-- Reply preview length before the card offers "More"; the GNOME card previews
-- the same 360 characters.
local PREVIEW_CHARS = 360

-- The panel surface is a fixed size (see plugin.toml). Reserving that height on
-- the root column is what lets the tab bodies flex and keeps the API key form
-- pinned above the bottom edge instead of being pushed out of view.
local PANEL_PADDING = 12
local PANEL_HEIGHT = 640
local CONTENT_HEIGHT = PANEL_HEIGHT - PANEL_PADDING * 2

local status = nil
local lastRevision = -1
local tab = "agent"
local providerIndex = 1
local activeProvider = ""
local activeModel = ""
local modelFilter = ""
local workflowFilter = ""
local expandedCard = false
-- "Copied", shown beside the card actions after a copy.
local copyNote = nil
local pairingNote = nil
local keyDraft = ""
local keyDraftProvider = nil
-- Set once the user picks a key provider by hand, so the form stops following
-- the active model's provider after that.
local keyTouched = false
local keyEpoch = 0
local savedKey = nil
-- Last painted countdown, so a second tick only re-renders when the digits changed.
local lastClock = nil
local openedAt = 0
-- Set from an async launch callback, which can run outside a UI callback; the
-- next second tick renders whatever it left behind.
local dirty = false

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

local function read_status()
  return decode((noctalia.readFile(STATUS)))
end

local function now_ms()
  return (noctalia.nowMs and noctalia.nowMs()) or 0
end

-- The agent labels a provider it has no credential for as "<provider> (key
-- needed)" so a bar-only desktop still shows where the gap is.
local function split_provider(name)
  local base = name:match("^(.-) %(key needed%)$")
  if base then
    return base, true
  end
  return name, false
end

local function catalogs()
  local entries = type(status) == "table" and status.catalogs or nil
  if type(entries) ~= "table" then
    return {}
  end
  local list = {}
  for _, entry in ipairs(entries) do
    local raw = type(entry.provider) == "string" and entry.provider or ""
    if raw ~= "" then
      local base, needsKey = split_provider(raw)
      list[#list + 1] = {
        provider = base,
        label = PROVIDER_LABELS[base] or base,
        needsKey = needsKey,
        fresh = entry.fresh ~= false,
        models = type(entry.models) == "table" and entry.models or {},
      }
    end
  end
  return list
end

local function find_catalog(base)
  for index, entry in ipairs(catalogs()) do
    if entry.provider == base then
      return entry, index
    end
  end
  return nil
end

local function shorten(text, limit)
  if type(text) ~= "string" then
    return nil
  end
  text = text:gsub("%s+", " ")
  if #text > limit then
    text = text:sub(1, limit - 1) .. "…"
  end
  return text ~= "" and text or nil
end

local function trim(text)
  if type(text) ~= "string" then
    return ""
  end
  if noctalia.string and noctalia.string.trim then
    return noctalia.string.trim(text)
  end
  return text:match("^%s*(.-)%s*$")
end

local function current_state()
  if type(status) == "table" and type(status.state) == "string" then
    return status.state:lower()
  end
  return "hidden"
end

local function state_status()
  local state = current_state()
  return STATE_STATUS[state] or STATE_STATUS.hidden
end

-- "12:34", or "1:02:03" once a countdown passes the hour.
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

-- A clock the reply itself mentions ("Focus session 25:00"), so a spoken
-- countdown still gets the big digits the GNOME card gives it.
local function clock_in(text)
  if type(text) ~= "string" then
    return nil
  end
  local start = 1
  while true do
    local from, to = text:find("%d%d?:%d%d", start)
    if not from then
      return nil
    end
    local before = from > 1 and text:sub(from - 1, from - 1) or ""
    local after = text:sub(to + 1, to + 1)
    if not before:match("%d") and not after:match("%d") and after ~= ":" then
      return text:sub(from, to)
    end
    start = to + 1
  end
end

-- The digits the Agent tab shows: a live timer the agent is running, or failing
-- that a clock in the reply itself. Returns the text, its caption when it has
-- one, and whether the countdown has run out — only a real timer can do that, so
-- a clock in a reply is never labelled "Finished".
local function clock_display()
  if type(status) ~= "table" then
    return nil
  end

  local timer = status.timer
  if type(timer) == "table" and type(timer.ends_at_ms) == "number" then
    local remaining = math.max(0, math.ceil((timer.ends_at_ms - now_ms()) / 1000))
    local label = type(timer.label) == "string" and shorten(timer.label, 80) or nil
    return format_seconds(remaining), label, remaining <= 0
  end

  local card = status.latest
  local body = type(card) == "table" and type(card.text) == "string" and card.text or nil
  local clock = body and clock_in(body) or nil
  if clock then
    return clock, nil, false
  end
  return nil
end

-- The agent ignores any control whose revision is not newer than the last one
-- it saw, so each action must advance it. nowMs keeps that monotonic across
-- panel reloads where the previous write is still on disk.
local function next_revision()
  local revision = 0
  local existing = decode((noctalia.readFile(CONTROL)))
  if existing and type(existing.revision) == "number" then
    revision = existing.revision
  end
  return math.max(now_ms(), revision + 1)
end

local function send(payload)
  payload.revision = next_revision()
  local encoded = noctalia.json.encode(payload)
  if not encoded then
    noctalia.notifyError("Hyusk", "Could not encode the request")
    return false
  end
  local ok, err = noctalia.writeFile(CONTROL, encoded)
  if not ok then
    noctalia.notifyError("Hyusk", err or "could not write " .. CONTROL)
  end
  return ok
end

local function config_dir()
  local configured = noctalia.getenv("XDG_CONFIG_HOME")
  if type(configured) == "string" and configured ~= "" then
    return configured
  end
  local home = noctalia.expandPath and noctalia.expandPath("~") or ""
  if home == "" then
    return "/tmp/.config"
  end
  return home .. "/.config"
end

local function workflows_path()
  return config_dir() .. "/hyusk/commands.json"
end

-- Named workflows from commands.json, the same file the agent's parser and the
-- workflow editor use. Read on every render so an editor save shows up at once.
local function workflows()
  local config = decode((noctalia.readFile(workflows_path())))
  local entries = type(config) == "table" and config.workflows or nil
  if type(entries) ~= "table" then
    return {}
  end
  local list = {}
  for _, entry in ipairs(entries) do
    local name = type(entry) == "table" and trim(entry.name) or ""
    if name ~= "" then
      local phrases = {}
      if type(entry.phrases) == "table" then
        for _, phrase in ipairs(entry.phrases) do
          if type(phrase) == "string" and trim(phrase) ~= "" then
            phrases[#phrases + 1] = trim(phrase)
          end
        end
      end
      list[#list + 1] = { name = name, phrases = phrases }
    end
  end
  return list
end

-- A companion script install-noctalia-plugin.sh copied next to this file.
local function plugin_script(name)
  if PLUGIN_DIR == "" then
    return nil
  end
  local path = PLUGIN_DIR .. "/" .. name
  if noctalia.fileExists and noctalia.fileExists(path) then
    return path
  end
  return nil
end

-- Open the native workflow studio, preferring the installed binary over the
-- location the installer uses, exactly as the GNOME indicator's launcher does.
local function launch_workflows_app()
  if noctalia.commandExists and noctalia.commandExists("hyusk-workflows") then
    if noctalia.runAsync({ "hyusk-workflows" }) then
      return
    end
  end
  local home = noctalia.expandPath and noctalia.expandPath("~") or ""
  local user_binary = home ~= "" and (home .. "/.local/bin/hyusk-workflows") or nil
  if user_binary and noctalia.fileExists(user_binary) and noctalia.runAsync({ user_binary }) then
    return
  end
  noctalia.notify("Hyusk Workflows", "Install or build the hyusk-workflows app first.")
end

-- Hyusk's dedicated CDP browser profile, separate from the normal one.
local function launch_automation_browser()
  local script = plugin_script("start-browser-control.sh")
  if not script then
    noctalia.notify(
      "Hyusk",
      "The automation browser script is missing. Rerun scripts/install-noctalia-plugin.sh."
    )
    return
  end
  if not noctalia.runAsync({ "bash", script }) then
    noctalia.notifyError("Hyusk", "Could not start the automation browser.")
  end
end

local function enable_laptop_link()
  local script = plugin_script("setup-mobile-link.sh")
  if not script then
    pairingNote = "Install the plugin with scripts/install-noctalia-plugin.sh first."
    render()
    return
  end
  pairingNote = "Enabling the secure laptop link…"
  render()
  noctalia.runAsync({ "bash", script, "--enable" }, function(result)
    -- This may run outside a UI callback, so leave the note and let the next
    -- second tick paint it.
    if result.exitCode ~= 0 then
      pairingNote = trim(result.stderr) ~= "" and trim(result.stderr)
        or "Could not enable the laptop link"
    else
      pairingNote = "Link enabled · generating a code…"
    end
    dirty = true
  end)
end

-- The key field targets whichever API provider the user last picked, so
-- switching model tabs carries the credential form along with it.
local function key_provider()
  for _, provider in ipairs(KEY_PROVIDERS) do
    if provider.id == keyDraftProvider then
      return provider
    end
  end
  return KEY_PROVIDERS[1]
end

local function section_title(text)
  return ui.label({ text = text, fontSize = 11, fontWeight = "semibold", color = "on_surface_variant" })
end

local function header()
  local children = {
    ui.glyph({ name = "butterfly", size = 20, color = "primary" }),
    ui.column({ flexGrow = 1, gap = 2 }, {
      ui.label({ text = "Hyusk", fontSize = 16, fontWeight = "bold" }),
      ui.label({
        text = (type(status) == "table" and current_state() or "offline") .. " · "
          .. ((activeProvider ~= "" and (PROVIDER_LABELS[activeProvider] or activeProvider) or "no model")),
        fontSize = 11,
        color = "on_surface_variant",
      }),
    }),
    ui.button({
      glyph = "refresh",
      variant = "ghost",
      tooltip = "Refresh the model list",
      onClick = function()
        send({ action = "refresh_models" })
      end,
    }),
  }
  if noctalia.getSetting("shell.show_close_buttons") ~= false then
    children[#children + 1] = ui.button({
      glyph = "close",
      variant = "ghost",
      onClick = function()
        panel.close()
      end,
    })
  end
  return ui.row({ align = "center", gap = 10 }, children)
end

local function tab_switcher()
  local buttons = {}
  for _, entry in ipairs(TABS) do
    buttons[#buttons + 1] = ui.button({
      key = "tab/" .. entry.id,
      text = entry.label,
      variant = tab == entry.id and "secondary" or "ghost",
      selected = tab == entry.id,
      controlSize = "sm",
      flexGrow = 1,
      onClick = function()
        if tab ~= entry.id then
          tab = entry.id
          render()
        end
      end,
    })
  end
  return ui.row({ gap = 4, align = "center" }, buttons)
end

-- Listen / Stop, plus the microphone-only stop the GNOME card offers while the
-- agent waits for a spoken reply.
local function transport()
  local state = current_state()
  local children = {
    ui.button({
      text = "Listen",
      variant = "primary",
      controlSize = "sm",
      flexGrow = 1,
      tooltip = "Start listening, exactly as the wake word does",
      onClick = function()
        send({ action = "start_listening" })
      end,
    }),
    ui.button({
      text = "Stop",
      variant = "destructive",
      controlSize = "sm",
      flexGrow = 1,
      tooltip = "Stop the running turn",
      onClick = function()
        send({ action = "stop_task" })
      end,
    }),
  }
  if state == "listening" or (type(status) == "table" and status.awaiting_reply == true) then
    children[#children + 1] = ui.button({
      text = "Stop listening",
      variant = "ghost",
      controlSize = "sm",
      flexGrow = 1,
      onClick = function()
        send({ action = "stop_listening" })
      end,
    })
  end
  return ui.row({ gap = 6, align = "center" }, children)
end

local function agent_body()
  local children = {}

  if type(status) ~= "table" then
    children[#children + 1] = ui.column({ gap = 4, padding = 10, fill = "surface_variant/0.4", radius = 8 }, {
      ui.label({ text = "Hyusk is not running.", fontWeight = "semibold" }),
      ui.label({
        text = "Start hyusk.service, then reopen this panel.",
        fontSize = 11,
        color = "on_surface_variant",
      }),
    })
    return ui.column({ gap = 10, flexGrow = 1 }, children)
  end

  children[#children + 1] = transport()

  local state_row = {
    ui.glyph({ name = "circle-filled", size = 10, color = "primary" }),
    ui.label({ text = state_status(), fontWeight = "semibold", flexGrow = 1 }),
  }
  if activeModel ~= "" then
    state_row[#state_row + 1] = ui.label({
      text = activeModel,
      fontSize = 11,
      color = "on_surface_variant",
      textAlign = "end",
      maxLines = 1,
    })
  end
  local card_children = { ui.row({ align = "center", gap = 8 }, state_row) }

  -- The countdown takes the card's centre, as it does in the GNOME card.
  local clock, clock_label, ended = clock_display()
  if clock then
    local clock_children = {
      ui.label({ text = clock, fontSize = 26, fontWeight = "bold", color = "primary" }),
    }
    if clock_label then
      clock_children[#clock_children + 1] = ui.label({
        text = clock_label,
        fontSize = 11,
        color = "on_surface_variant",
        maxLines = 1,
      })
    end
    if ended then
      clock_children[#clock_children + 1] = ui.label({
        text = "Finished",
        fontSize = 11,
        color = "on_surface_variant",
      })
    end
    card_children[#card_children + 1] = ui.column({
      gap = 2,
      align = "center",
      padding = 8,
      fill = "surface_variant/0.35",
      radius = 8,
    }, clock_children)
  end

  local card = status.latest
  local raw = type(card) == "table" and type(card.text) == "string" and card.text or nil
  local body = raw and shorten(raw, expandedCard and 4000 or PREVIEW_CHARS) or nil
  if body then
    card_children[#card_children + 1] = ui.label({
      text = body,
      fontSize = 12,
      color = card.kind == "error" and "error" or "on_surface_variant",
      maxLines = expandedCard and 24 or 6,
    })
  end

  if type(card) == "table" and card.kind == "approval" then
    card_children[#card_children + 1] = ui.row({ gap = 6, align = "center" }, {
      ui.button({
        text = "Approve",
        variant = "primary",
        controlSize = "sm",
        flexGrow = 1,
        onClick = function()
          send({ action = "approve" })
        end,
      }),
      ui.button({
        text = "Deny",
        variant = "ghost",
        controlSize = "sm",
        flexGrow = 1,
        onClick = function()
          send({ action = "deny" })
        end,
      }),
    })
  end

  if status.awaiting_reply == true then
    card_children[#card_children + 1] = ui.label({
      text = "Waiting for your reply",
      fontSize = 11,
      color = "primary",
    })
  end

  if body or (type(card) == "table" and card.kind == "approval") then
    local actions = {}
    if raw and #raw > PREVIEW_CHARS then
      actions[#actions + 1] = ui.button({
        text = expandedCard and "Less" or "More",
        variant = "ghost",
        controlSize = "sm",
        onClick = function()
          expandedCard = not expandedCard
          render()
        end,
      })
    end
    actions[#actions + 1] = ui.button({
      text = "Copy",
      variant = "ghost",
      controlSize = "sm",
      tooltip = "Copy the reply to the clipboard",
      onClick = function()
        local text = raw or ""
        if text ~= "" and noctalia.copyToClipboard(text, "text/plain") then
          copyNote = "Copied to the clipboard."
          render()
        end
      end,
    })
    actions[#actions + 1] = ui.button({
      text = "Dismiss",
      variant = "ghost",
      controlSize = "sm",
      onClick = function()
        send({ action = "dismiss_card" })
      end,
    })
    card_children[#card_children + 1] = ui.row({ gap = 6, align = "center" }, actions)
  end

  if copyNote then
    card_children[#card_children + 1] = ui.label({ text = copyNote, fontSize = 11, color = "primary" })
  end

  children[#children + 1] = ui.column({
    gap = 8,
    padding = 10,
    fill = "surface_variant/0.35",
    radius = 8,
  }, card_children)

  return ui.scroll({ gap = 10, flexGrow = 1 }, children)
end

local function model_rows(entry)
  local rows = {}
  local filter = modelFilter:lower()
  -- The active model leads the list: otherwise a 467-model catalog buries the
  -- current choice under 400 rows of scrollback.
  local ordered = {}
  for _, model in ipairs(entry.models) do
    if entry.provider == activeProvider and model.id == activeModel then
      table.insert(ordered, 1, model)
    else
      ordered[#ordered + 1] = model
    end
  end

  local matches = 0
  for _, model in ipairs(ordered) do
    local id = type(model.id) == "string" and model.id or ""
    local label = type(model.label) == "string" and model.label or id
    if filter == "" or label:lower():find(filter, 1, true) or id:lower():find(filter, 1, true) then
      matches = matches + 1
      if #rows < MAX_MODEL_ROWS then
        local selected = entry.provider == activeProvider and id == activeModel
        rows[#rows + 1] = ui.button({
          key = entry.provider .. "/" .. id,
          text = label,
          variant = selected and "primary" or "ghost",
          contentAlign = "start",
          controlSize = "sm",
          tooltip = id,
          onClick = function()
            -- A press delivered while the panel is still materialising under the
            -- pointer is a stray click, not a choice: opening the panel must not
            -- silently switch the model.
            if now_ms() - openedAt < 400 then
              return
            end
            activeProvider = entry.provider
            activeModel = id
            send({ action = "set_model", provider = entry.provider, model = id })
            render()
          end,
        })
      end
    end
  end
  if matches == 0 then
    rows[#rows + 1] = ui.label({
      text = "No model matches that filter.",
      fontSize = 11,
      color = "on_surface_variant",
    })
  elseif matches > #rows then
    rows[#rows + 1] = ui.label({
      text = string.format("Showing %d of %d matches — narrow the filter.", #rows, matches),
      fontSize = 11,
      color = "on_surface_variant",
    })
  end
  return rows
end

-- Returns the model picker's controls and its scrollable body, so the key form
-- below keeps its space no matter how many models a provider advertises.
local function model_body()
  local entries = catalogs()
  if #entries == 0 then
    return nil, ui.column({ gap = 6, padding = 8 }, {
      ui.label({
        text = "No catalog yet. Refresh once Hyusk has a key.",
        fontSize = 11,
        color = "on_surface_variant",
      }),
    })
  end

  if providerIndex < 1 or providerIndex > #entries then
    providerIndex = 1
  end
  local entry = entries[providerIndex]
  local options = {}
  for _, item in ipairs(entries) do
    options[#options + 1] = item.needsKey and (item.label .. " · key needed") or item.label
  end

  local controls = {
    section_title("MODEL"),
    ui.select({
      options = options,
      selectedIndex = providerIndex - 1,
      onChange = function(index)
        providerIndex = (tonumber(index) or 0) + 1
        modelFilter = ""
        local chosen = catalogs()[providerIndex]
        if chosen then
          for _, provider in ipairs(KEY_PROVIDERS) do
            if provider.id == chosen.provider then
              keyDraftProvider = chosen.provider
              keyDraft = ""
            end
          end
        end
        render()
      end,
    }),
  }

  if entry.needsKey then
    return ui.column({ gap = 8 }, controls), ui.column({ gap = 6, padding = 8 }, {
      ui.label({
        text = "Add an " .. entry.label .. " key below to load its models.",
        fontSize = 11,
        color = "on_surface_variant",
      }),
    })
  end

  controls[#controls + 1] = ui.input({
    key = "model-filter-" .. entry.provider,
    placeholder = entry.fresh and "Filter models" or "Filter models · cached list",
    controlSize = "sm",
    onChange = function(value)
      modelFilter = value
      render()
    end,
  })
  return ui.column({ gap = 8 }, controls), ui.scroll({ flexGrow = 1, gap = 2 }, model_rows(entry))
end

local function workflows_body()
  local entries = workflows()
  local children = {
    ui.row({ align = "center", gap = 8 }, {
      section_title("WORKFLOWS"),
      ui.label({
        text = #entries == 1 and "1 saved" or (#entries .. " saved"),
        fontSize = 11,
        color = "on_surface_variant",
        flexGrow = 1,
      }),
      ui.button({
        text = "Open editor",
        variant = "ghost",
        controlSize = "sm",
        tooltip = "Open the hyusk-workflows app",
        onClick = launch_workflows_app,
      }),
    }),
  }

  local filter = workflowFilter:lower()
  local rows = {}
  local matches = 0
  for _, workflow in ipairs(entries) do
    local searchable = workflow.name:lower()
    for _, phrase in ipairs(workflow.phrases) do
      searchable = searchable .. " " .. phrase:lower()
    end
    if filter == "" or searchable:find(filter, 1, true) then
      matches = matches + 1
      if #rows < MAX_WORKFLOW_ROWS then
        local tooltip_text = #workflow.phrases > 0
          and table.concat(workflow.phrases, " · ")
          or "Run this workflow"
        rows[#rows + 1] = ui.button({
          key = "workflow/" .. workflow.name,
          text = workflow.name,
          variant = "ghost",
          contentAlign = "start",
          controlSize = "sm",
          flexGrow = 1,
          tooltip = tooltip_text,
          onClick = function()
            send({ action = "run_workflow", workflow = workflow.name })
          end,
        })
      end
    end
  end

  if #entries == 0 then
    children[#children + 1] = ui.label({
      text = "No workflows yet — create one in the editor.",
      fontSize = 11,
      color = "on_surface_variant",
    })
    return ui.column({ gap = 10, flexGrow = 1 }, children)
  end

  children[#children + 1] = ui.input({
    key = "workflow-filter",
    placeholder = "Filter workflows by name or phrase",
    controlSize = "sm",
    onChange = function(value)
      workflowFilter = value
      render()
    end,
  })

  if matches == 0 then
    rows[#rows + 1] = ui.label({
      text = "No workflow matches that filter.",
      fontSize = 11,
      color = "on_surface_variant",
    })
  elseif matches > #rows then
    rows[#rows + 1] = ui.label({
      text = string.format("Showing %d of %d matches — narrow the filter.", #rows, matches),
      fontSize = 11,
      color = "on_surface_variant",
    })
  end

  children[#children + 1] = ui.scroll({ gap = 2, flexGrow = 1 }, rows)

  return ui.column({ gap = 10, flexGrow = 1 }, children)
end

local function device_body()
  local children = {}
  local pairing_raw = noctalia.readFile(PAIRING)
  local pairing = decode(pairing_raw)
  local now = math.floor(now_ms() / 1000)
  local valid = type(pairing) == "table"
    and pairing.protocol == "hyusk.link.v1"
    and type(pairing.secret) == "string" and pairing.secret ~= ""
    and type(pairing.expires_at) == "number" and pairing.expires_at > now

  children[#children + 1] = ui.column({ gap = 8, padding = 10, fill = "surface_variant/0.35", radius = 8 }, {
    section_title("PHONE"),
    ui.label({
      text = valid and "One-time code · expires in "
          .. math.max(1, math.ceil((pairing.expires_at - now) / 60)) .. " min"
        or (pairing and "Code expired · generate a fresh one" or "Link is off · enable the laptop link first"),
      fontSize = 11,
      color = valid and "primary" or "on_surface_variant",
    }),
  })

  local phone_actions = {}
  if valid then
    phone_actions[#phone_actions + 1] = ui.button({
      text = "Copy code",
      variant = "primary",
      controlSize = "sm",
      flexGrow = 1,
      tooltip = "Copy the pairing code for the phone's Devices page",
      onClick = function()
        if pairing_raw and noctalia.copyToClipboard(pairing_raw, "text/plain") then
          pairingNote = "Pairing code copied."
          render()
        end
      end,
    })
  else
    phone_actions[#phone_actions + 1] = ui.button({
      text = "Enable laptop link",
      variant = "primary",
      controlSize = "sm",
      flexGrow = 1,
      onClick = enable_laptop_link,
    })
  end
  phone_actions[#phone_actions + 1] = ui.button({
    text = "Generate fresh code",
    variant = "ghost",
    controlSize = "sm",
    flexGrow = 1,
    onClick = function()
      send({ action = "pair_phone" })
    end,
  })
  children[#children + 1] = ui.row({ gap = 6, align = "center" }, phone_actions)

  if valid and pairing_raw then
    children[#children + 1] = ui.label({
      text = shorten(pairing_raw, 400),
      fontSize = 10,
      color = "on_surface_variant",
      maxLines = 4,
    })
  end

  if pairingNote then
    children[#children + 1] = ui.label({ text = pairingNote, fontSize = 11, color = "primary", maxLines = 3 })
  end

  children[#children + 1] = ui.label({
    text = "Codes work once and expire after five minutes; both devices must reach this laptop on port 4488.",
    fontSize = 11,
    color = "on_surface_variant",
    maxLines = 4,
  })

  children[#children + 1] = ui.column({ gap = 8, padding = 10, fill = "surface_variant/0.35", radius = 8 }, {
    section_title("DESKTOP"),
    ui.button({
      text = "Open automation browser",
      variant = "ghost",
      controlSize = "sm",
      flexGrow = 1,
      onClick = launch_automation_browser,
    }),
    ui.label({
      text = "Hyusk's own browser profile for CDP automation, with its own cookies and sign-ins.",
      fontSize = 11,
      color = "on_surface_variant",
      maxLines = 4,
    }),
  })

  return ui.scroll({ gap = 10, flexGrow = 1 }, children)
end

local function tab_body()
  if tab == "models" then
    local controls, list = model_body()
    local children = {}
    if controls then
      children[#children + 1] = controls
    end
    if list then
      children[#children + 1] = list
    end
    return ui.column({ gap = 10, flexGrow = 1 }, children)
  end
  if tab == "workflows" then
    return workflows_body()
  end
  if tab == "device" then
    return device_body()
  end
  return agent_body()
end

local function key_section()
  local target = key_provider()
  local entry = find_catalog(target.id)
  local saved = savedKey == target.id or (entry and entry.needsKey == false)
  local children = {
    section_title("API KEY"),
    ui.row({ align = "center", gap = 6 }, {
      ui.label({ text = target.label, fontWeight = "semibold", flexGrow = 1 }),
      ui.label({
        text = saved and "Configured" or "Missing",
        fontSize = 11,
        color = saved and "primary" or "error",
      }),
    }),
    ui.row({ gap = 6, align = "center" }, {
      ui.input({
        key = string.format("api-key-%s-%d", target.id, keyEpoch),
        placeholder = "Paste key",
        password = true,
        controlSize = "sm",
        flexGrow = 1,
        onChange = function(value)
          keyDraft = value
        end,
        onSubmit = function(value)
          keyDraft = value
          onSaveKey()
        end,
      }),
      ui.button({
        text = "Save",
        variant = "primary",
        onClick = function()
          onSaveKey()
        end,
      }),
    }),
    ui.label({
      text = savedKey == target.id
        and "Stored in your login keyring. Reloading its models…"
        or "Stored in your login keyring, never in this panel's config.",
      fontSize = 11,
      color = savedKey == target.id and "primary" or "on_surface_variant",
    }),
  }

  -- Keys are per provider, so switching the model tab switches the key field.
  local others = {}
  for _, provider in ipairs(KEY_PROVIDERS) do
    if provider.id ~= target.id then
      others[#others + 1] = ui.button({
        text = provider.label,
        variant = "ghost",
        controlSize = "sm",
        onClick = function()
          keyDraftProvider = provider.id
          keyTouched = true
          keyDraft = ""
          render()
        end,
      })
    end
  end
  if #others > 0 then
    local row = { ui.label({ text = "Other:", fontSize = 11, color = "on_surface_variant" }) }
    for _, button in ipairs(others) do
      row[#row + 1] = button
    end
    children[#children + 1] = ui.row({ gap = 4, align = "center" }, row)
  end
  return ui.column({ gap = 8 }, children)
end

function onSaveKey()
  local target = key_provider()
  local key = trim(keyDraft)
  if key == "" then
    noctalia.notifyError("Hyusk", "Paste an API key before saving")
    return
  end
  send({ action = "set_api_key", provider = target.id, key = key })
  keyDraft = ""
  keyEpoch = keyEpoch + 1
  savedKey = target.id
  -- A fresh key means the agent is already fetching the new catalog.
  render()
end

function render()
  local children = {
    header(),
    tab_switcher(),
    tab_body(),
    ui.separator({}),
    key_section(),
  }
  panel.render(ui.column({
    gap = 10,
    padding = PANEL_PADDING,
    height = CONTENT_HEIGHT,
  }, children))
  local clock = clock_display()
  lastClock = clock
end

function onOpen(_context)
  status = read_status()
  lastRevision = type(status) == "table" and status.revision or -1
  if type(status) == "table" then
    activeProvider = type(status.active_provider) == "string" and status.active_provider or activeProvider
    activeModel = type(status.active_model) == "string" and status.active_model or activeModel
    for index, entry in ipairs(catalogs()) do
      if entry.provider == activeProvider then
        providerIndex = index
        break
      end
    end
  end
  if not keyTouched then
    keyDraftProvider = KEY_PROVIDERS[1].id
    for _, provider in ipairs(KEY_PROVIDERS) do
      if provider.id == activeProvider then
        keyDraftProvider = provider.id
      end
    end
  end
  expandedCard = false
  copyNote = nil
  pairingNote = nil
  openedAt = now_ms()
  -- Second boundaries drive both the status poll and a live countdown; they stop
  -- on close and resume on the next open.
  panel.setWantsSecondTicks(true)
  render()
end

function onClose()
  panel.setWantsSecondTicks(false)
end

-- The agent rewrites the status file on every state change, so a once-a-second
-- poll follows it; a timer only needs a repaint when its digits change, and only
-- the Agent tab shows them. The poll runs first: a countdown's tick must never
-- starve it, or a reply would wait behind a running timer.
function update()
  if dirty then
    dirty = false
    render()
    return
  end

  local fresh = read_status()
  local revision = type(fresh) == "table" and fresh.revision or -1
  if revision ~= lastRevision then
    status = fresh
    lastRevision = revision
    if type(fresh) == "table" then
      activeProvider = type(fresh.active_provider) == "string" and fresh.active_provider or activeProvider
      activeModel = type(fresh.active_model) == "string" and fresh.active_model or activeModel
    end
    -- A new payload is a new card, so drop the previous "Copied"/"Less" state.
    expandedCard = false
    copyNote = nil
    render()
    return
  end

  if tab == "agent" then
    local clock = clock_display()
    if clock ~= lastClock then
      lastClock = clock
      render()
    end
  end
end
