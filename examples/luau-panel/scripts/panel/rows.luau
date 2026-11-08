--!strict
local ui = require("@rune/ui")

export type Item = { id: number, name: string, size: number, done: boolean }

local NAMES = { "alpha", "bravo", "charlie", "delta", "echo", "foxtrot", "golf", "hotel", "india", "juliet" }

local function items(n: number): { Item }
	local out = table.create(n)
	for i = 1, n do
		out[i] = { id = i, name = `{NAMES[(i - 1) % #NAMES + 1]} {i}`, size = (i * 7919) % 1000, done = i % 3 == 0 }
	end
	return out
end

-- Reorders sizes deterministically for round `seed`.
local function shuffle(list: { Item }, seed: number)
	for i, item in list do
		item.size = (i * 7919 + seed * 104729) % 1000
	end
end

local function row(item: Item, selected: boolean, onPress: () -> ())
	return ui.row({
		key = item.id,
		w = "full",
		h = 28,
		px = 16,
		gap = 10,
		align = "center",
		fill = if selected then "control-off" else nil,
		hover = "hover",
		onPress = onPress,
	}, {
		ui.text(`#{item.id}`, { size = 11, mono = true, color = "text-secondary", w = 44 }),
		ui.text(item.name, { size = 13, grow = 1, truncate = true }),
		ui.box({ w = 100, h = 4, radius = 2, fill = "separator" }, {
			ui.box({ w = item.size / 10, h = 4, radius = 2, fill = ui.hex("#009fff") }),
		}),
		ui.text(if item.done then "done" else "open", {
			size = 11,
			color = if item.done then ui.hex("#30a46c") else "text-secondary",
			w = 36,
		}),
	})
end

local LABELS = {
	"Session", "Messages", "Provider", "Model", "Context limit", "Total tokens", "Usage", "Input tokens",
	"Output tokens", "Reasoning tokens", "Cache read", "Cache write", "User messages", "Assistant messages",
	"Total cost", "Created", "Last activity", "System prompt", "Tools", "Files",
}

-- Twenty label/value/bar rows, values computed per build as a stats tab's would be.
local function panel20(seed: number)
	local out = table.create(20)
	for i = 1, 20 do
		local value = (i * 7919 + seed * 31) % 100000
		out[i] = ui.row({ key = i, w = "full", h = 24, px = 16, gap = 8, align = "center" }, {
			ui.text(LABELS[i], { size = 12, color = "text-secondary", w = 140 }),
			ui.text(`{value // 1000},{string.format("%03d", value % 1000)}`, { size = 12, grow = 1, mono = true }),
			ui.box({ w = 80, h = 4, radius = 2, fill = "separator" }, {
				ui.box({ w = (value % 100) * 0.8, h = 4, radius = 2, fill = ui.hex("#009fff") }),
			}),
		})
	end
	return ui.col({ key = "panel20", w = "full", py = 8 }, out)
end

return {
	items = items,
	shuffle = shuffle,
	row = row,
	panel20 = panel20,
}
