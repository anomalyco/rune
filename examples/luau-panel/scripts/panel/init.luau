--!strict
-- A panel a Rune app hosts: a header that counts presses, a pulse driven by task.wait, a
-- signal, and a 1,000-row virtual list. Edit this file while the app runs: it reloads in place
-- and keeps the selection and counters (saveHotState / loadHotState).
local ui = require("@rune/ui")
local rows = require("@self/rows")

local state = {
	selected = 0,
	presses = 0,
	pulse = 0,
	pings = 0,
	shuffles = 0,
}

local items = rows.items(1000)
local listVersion = 0
local headerVersion = 0
local pulsing: thread? = nil
local pinged = require("@rune/signal").new()

local function changed(list: boolean)
	headerVersion += 1
	if list then
		listVersion += 1
	end
end

pinged:Connect(function(n: number)
	state.pings = n
	changed(false)
end)

local function select(i: number)
	state.selected = i
	state.presses += 1
	changed(true)
end

local function pulse()
	if pulsing then
		task.cancel(pulsing)
	end
	pulsing = task.spawn(function()
		for step = 10, 0, -1 do
			state.pulse = step
			changed(false)
			task.wait(0.08)
		end
		pulsing = nil
	end)
end

local function button(key: string, label: string, onPress: () -> ())
	return ui.row({
		key = key,
		px = 10,
		py = 4,
		radius = 6,
		fill = "control-off",
		hover = "hover",
		press = "press",
		onPress = onPress,
	}, { ui.text(label, { size = 12, weight = "medium" }) })
end

local function header()
	local selected = if state.selected > 0 then `#{state.selected}` else "none"
	return ui.col({ key = "header", w = "full", px = 16, pt = 14, pb = 10, gap = 8 }, {
		ui.row({ w = "full", gap = 8, align = "center" }, {
			ui.text("Luau panel", { size = 15, weight = "semibold" }),
			ui.spacer(),
			ui.text(`{#items} rows · selected {selected} · {state.presses} presses`, { size = 12, color = "text-secondary" }),
		}),
		ui.row({ w = "full", gap = 6, align = "center" }, {
			button("shuffle", "Shuffle", function()
				state.shuffles += 1
				rows.shuffle(items, state.shuffles)
				changed(true)
			end),
			button("pulse", "Pulse", pulse),
			button("ping", "Ping", function()
				pinged:Fire(state.pings + 1)
			end),
			ui.spacer(),
			ui.box({ w = 120, h = 6, radius = 3, fill = "separator" }, {
				ui.box({ w = state.pulse * 12, h = 6, radius = 3, fill = ui.hex("#009fff") }),
			}),
			ui.text(`{state.pings} pings`, { size = 12, color = "text-secondary", w = 60 }),
		}),
	})
end

local function render(view)
	return ui.col({ key = "panel", w = "full", h = "full", fill = "bg" }, {
		view:Memo("header", headerVersion, header),
		ui.box({ w = "full", h = 1, fill = "separator" }),
		ui.virtualList({
			key = "items",
			w = "full",
			grow = 1,
			count = #items,
			estimatedRowHeight = 28,
			version = listVersion,
			renderRow = function(i: number)
				return rows.row(items[i], i == state.selected, function()
					select(i)
				end)
			end,
		}),
	})
end

return {
	render = render,
	-- For the host: select a row, and read the selection.
	select = select,
	selected = function()
		return state.selected
	end,
	-- A 20-row stateful panel: the rebuild the performance gate measures.
	panel20 = function(view)
		return rows.panel20(state.presses)
	end,
	saveHotState = function()
		return state
	end,
	loadHotState = function(saved)
		for k, v in saved do
			state[k] = v
		end
		rows.shuffle(items, state.shuffles)
	end,
}
