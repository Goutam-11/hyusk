-- Hyusk settings panel — the [[panel]] entry behind the bar widget's gear.
--
-- The agent stays the only writer of its own configuration: this panel reads
-- $XDG_RUNTIME_DIR/hyusk-status.json (state, active model, model catalogs) and
-- writes requests to hyusk-control.json, exactly as the GNOME indicator does.
-- A pasted API key is handed to the agent, which stores it in the login keyring
-- and immediately overwrites the control file so the key does not linger there.

local function runtime_dir()
  return noctalia.getenv("XDG_RUNTIME_DIR") or "/tmp"
end

local STATUS = runtime_dir() .. "/hyusk-status.json"
local CONTROL = runtime_dir() .. "/hyusk-control.json"

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

-- Ceiling on rendered rows: the list scrolls, but a catalog of several hundred
-- models should not build several hundred controls on every render.
local MAX_MODEL_ROWS = 200

-- The panel surface is a fixed size (see plugin.toml). Reserving that height on
-- the root column is what lets the model list flex and keeps the API key form
-- pinned above the bottom edge instead of being pushed out of view.
local PANEL_PADDING = 12
local PANEL_HEIGHT = 640
local CONTENT_HEIGHT = PANEL_HEIGHT - PANEL_PADDING * 2

local status = nil
local lastRevision = -1
local providerIndex = 1
local activeProvider = ""
local activeModel = ""
local modelFilter = ""
local keyDraft = ""
local keyDraftProvider = nil
-- Set once the user picks a key provider by hand, so the form stops following
-- the active model's provider after that.
local keyTouched = false
local keyEpoch = 0
local savedKey = nil
local lastPoll = 0
local openedAt = 0

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
  return decode(noctalia.readFile(STATUS))
end

-- The agent labels a provider it has no credential for as "<provider> (key
-- needed)" so a bar-only desktop still shows where the gap is.
local function split_provider(name)
  local base, needs = name:match("^(.-) %(key needed%)$")
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

-- The agent ignores any control whose revision is not newer than the last one
-- it saw, so each action must advance it. nowMs keeps that monotonic across
-- panel reloads where the previous write is still on disk.
local function next_revision()
  local revision = 0
  local existing = decode(noctalia.readFile(CONTROL))
  if existing and type(existing.revision) == "number" then
    revision = existing.revision
  end
  local now = (noctalia.nowMs and noctalia.nowMs()) or 0
  return math.max(now, revision + 1)
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
        text = (type(status) == "table" and status.state or "offline") .. " · "
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

local function status_card()
  if type(status) ~= "table" then
    return ui.column({ gap = 4, padding = 10, fill = "surface_variant/0.4", radius = 8 }, {
      ui.label({ text = "Hyusk is not running.", fontWeight = "semibold" }),
      ui.label({
        text = "Start hyusk.service, then reopen this panel.",
        fontSize = 11,
        color = "on_surface_variant",
      }),
    })
  end
  local card = status.latest
  local text = type(card) == "table" and shorten(card.text, 260) or nil
  if not text and status.awaiting_reply ~= true then
    return nil
  end
  local lines = {}
  if text then
    lines[#lines + 1] = ui.label({
      text = text,
      fontSize = 11,
      color = card.kind == "error" and "error" or "on_surface_variant",
    })
  end
  if status.awaiting_reply == true then
    lines[#lines + 1] = ui.label({ text = "Waiting for your reply", fontSize = 11, color = "primary" })
  end
  return ui.column({ gap = 4, padding = 10, fill = "surface_variant/0.35", radius = 8 }, lines)
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
            if ((noctalia.nowMs and noctalia.nowMs()) or 0) - openedAt < 400 then
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
local function model_section(entries)
  if #entries == 0 then
    return ui.column({ gap = 6 }, {
      section_title("MODEL"),
      ui.label({
        text = "No catalog yet. Refresh once Hyusk has a key.",
        fontSize = 11,
        color = "on_surface_variant",
      }),
    }), nil
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

local function key_section(entries)
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
  local key = (keyDraft or ""):match("^%s*(.-)%s*$")
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
  local entries = catalogs()
  local controls, body = model_section(entries)
  local children = {
    header(),
  }
  local card = status_card()
  if card then
    children[#children + 1] = card
  end
  children[#children + 1] = ui.separator({})
  children[#children + 1] = controls
  if body then
    children[#children + 1] = body
  end
  children[#children + 1] = ui.separator({})
  children[#children + 1] = key_section(entries)
  panel.render(ui.column({
    gap = 10,
    padding = PANEL_PADDING,
    height = CONTENT_HEIGHT,
  }, children))
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
  openedAt = (noctalia.nowMs and noctalia.nowMs()) or 0
  panel.setNeedsFrameTick(true)
  render()
end

function onClose()
  panel.setNeedsFrameTick(false)
end

-- The agent rewrites the status file on every state change. Poll it about once
-- a second while the panel is open instead of asking the host for every frame.
function onFrameTick(_delta_ms)
  local now = (noctalia.nowMs and noctalia.nowMs()) or 0
  if now - lastPoll < 900 then
    return
  end
  lastPoll = now
  local fresh = read_status()
  local revision = type(fresh) == "table" and fresh.revision or -1
  if revision ~= lastRevision then
    status = fresh
    lastRevision = revision
    if type(fresh) == "table" then
      activeProvider = type(fresh.active_provider) == "string" and fresh.active_provider or activeProvider
      activeModel = type(fresh.active_model) == "string" and fresh.active_model or activeModel
    end
    render()
  end
end
