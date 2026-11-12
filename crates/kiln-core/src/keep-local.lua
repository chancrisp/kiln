-- Kiln: Pandoc runs with --sandbox, which stops it reading any file a
-- document names, its pictures too. This keeps the pictures that sit in the
-- input file's own folder (or a folder in it) by reading them here, which
-- the sandbox doesn't cover, and handing them to the media bag the writers
-- take pictures from. Every other picture becomes its alt text.

local BS = string.char(92)
local MAX = 64 * 1024 * 1024
local MIME = {
  png = "image/png", jpg = "image/jpeg", jpeg = "image/jpeg", gif = "image/gif",
  webp = "image/webp", bmp = "image/bmp", tif = "image/tiff", tiff = "image/tiff",
}

-- The folder of the input file, written with / and with each % as %25: Pandoc
-- reads %XX in the path it is given as a character, so a folder with %41 in
-- its name would become another folder. A \\?\ prefix is dropped. `nil` for a
-- network folder (\\server\share): given a path like that, Pandoc asks
-- http://server for the file instead of reading it.
local function folder_of(path)
  if not path then return "." end
  path = path:gsub(BS, "/")
  if path:sub(1, 4) == "//?/" then
    path = path:sub(5)
    if path:sub(1, 4):upper() == "UNC/" then path = "//" .. path:sub(5) end
  end
  if path:sub(1, 2) == "//" then return nil end
  local dir = path:match("^(.*)/[^/]*$")
  if not dir then return "." end
  return (dir:gsub("%%", "%%25"))
end

local root = folder_of(PANDOC_STATE.input_files[1])

local function decode(s)
  return (s:gsub("%%(%x%x)", function(h) return string.char(tonumber(h, 16)) end))
end

local function extension(rel)
  return (rel:match("%.(%w+)$") or ""):lower()
end

-- Whether a path is relative and stays inside the folder it is relative to.
-- A colon is a drive, a scheme or a stream name.
local function inside(rel)
  return not rel:find("%c")
    and not rel:find("..", 1, true)
    and not rel:find(":", 1, true)
    and not rel:match("^[/" .. BS .. "]")
end

local verdicts = {}

-- Whether the picture at `src` stays. A local one is read and put in the
-- media bag under the name the document gives it.
local function keep(src)
  if verdicts[src] ~= nil then return verdicts[src] end
  local result = false
  if src:sub(1, 5):lower() == "data:" or pandoc.mediabag.lookup(src) then
    result = true
  else
    -- Pandoc reads %XX in the path it opens, so the path must be inside as
    -- written and after each reading of it (the second is for %25XX).
    local once = decode(src)
    local forms = { src, once, decode(once) }
    local admitted = true
    for _, rel in ipairs(forms) do
      if not inside(rel) or not MIME[extension(rel)] then admitted = false end
    end
    if admitted and root then
      local ok, _, data = pcall(pandoc.mediabag.fetch, root .. "/" .. src)
      if ok and type(data) == "string" and #data <= MAX then
        pandoc.mediabag.insert(src, MIME[extension(src)], data)
        result = true
      end
    end
  end
  verdicts[src] = result
  return result
end

local function Image(img)
  if keep(img.src) then return img end
  return img.caption
end

-- Markdown makes a figure of a picture on a line of its own, with its alt
-- text as the caption. With the picture gone, the alt text would show twice
-- (as the figure's body and as its caption), so the figure is the alt text.
local function Figure(fig)
  local body = #fig.content == 1 and fig.content[1]
  if not body or (body.tag ~= "Plain" and body.tag ~= "Para") then return nil end
  local img = #body.content == 1 and body.content[1]
  if not img or img.tag ~= "Image" or keep(img.src) then return nil end
  local alt = pandoc.utils.stringify(img.caption)
  if alt ~= pandoc.utils.stringify(fig.caption.long) then return nil end
  if alt == "" then return {} end
  return pandoc.Para(img.caption)
end

-- Top down, so a figure is seen before the picture in it.
return { { traverse = "topdown", Figure = Figure, Image = Image } }
