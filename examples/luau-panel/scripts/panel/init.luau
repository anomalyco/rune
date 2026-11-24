--!strict
-- A panel a Rune app hosts: a header that counts presses, a pulse driven by task.wait, a
-- signal, and a 1,000-row virtual list. Edit this file while the app runs: it reloads in place
-- and keeps the selection and counters (saveHotState / loadHotState).
local ui = require("@rune/ui")
local rows = require("@self/rows")
local showcase = require("@self/showcase")

local state = {
	selected = 0,
	presses = 0,
	pulse = 0,
	pings = 0,
	shuffles = 0,
	tab = "list",
	pill = 1,
	cardSlot = 0,
	scrollY = 0,
	scrollMaxY = 0,
	scrollFy = 0,
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
		pressScale = 0.96,
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
			button("tab-toggle", if state.tab == "list" then "Props" else "List", function()
				state.tab = if state.tab == "list" then "props" else "list"
				changed(false)
			end),
			ui.spacer(),
			ui.box({ w = 120, h = 6, radius = 3, fill = "separator", role = "progressbar", label = "Pulse progress" }, {
				ui.box({ w = `{state.pulse * 10}%`, h = "100%", radius = 3, fill = ui.hex("#009fff") }),
			}),
			ui.text(`{state.pings} pings`, { size = 12, color = "text-secondary", w = 60 }),
		}),
	})
end

local function render(view)
	local body
	if state.tab == "props" then
		body = showcase.render(
			state.pill,
			state.cardSlot,
			view.reducedMotion,
			state.scrollY,
			state.scrollMaxY,
			state.scrollFy,
			function(i: number)
				state.pill = i
				changed(false)
			end,
			function()
				state.cardSlot = 1 - state.cardSlot
				changed(false)
			end,
			function(e)
				state.scrollY = e.y
				state.scrollMaxY = e.maxY
				state.scrollFy = e.fy
			end
		)
	else
		body = ui.virtualList({
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
		})
	end
	return ui.col({ key = "panel", w = "full", h = "full", fill = "bg" }, {
		view:Memo("header", headerVersion, header),
		ui.box({ w = "full", h = 1, fill = "separator" }),
		body,
	})
end

return {
	render = render,
	-- For the host: select a row, switch tabs, and read the selection.
	select = select,
	setTab = function(tab: string)
		state.tab = tab
		changed(false)
	end,
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
