--!strict
-- Showcase of rune-luau layout, transform, outline, clip, line-clamp, matched-flight, and ARIA props.
local ui = require("@rune/ui")

local CARD = table.freeze({
	pad = 12,
	gap = 8,
	radius = 8,
	fill = "card",
	border = "card-border",
	borderWidth = 1,
})

local SAMPLE_TEXT = "Shared-element flights, line clamping, equal-column grids, percentage sizes, rounded clips, outlines, and 2D transforms all map directly onto Rune El builders from Luau without a custom host node."

local PILLS = { "Alpha", "Bravo", "Charlie" }

local function render(
	activePill: number,
	cardSlot: number,
	reducedMotion: boolean,
	scrollY: number,
	scrollMaxY: number,
	scrollFy: number,
	onPill: (number) -> (),
	onSwap: () -> (),
	onScroll: (any) -> ()
)
	local tabChildren = table.create(#PILLS)
	for i, label in PILLS do
		local selected = i == activePill
		tabChildren[i] = ui.row({
			key = `pill-{i}`,
			role = "tab",
			label = label,
			px = 12,
			py = 6,
			radius = 6,
			pressScale = 0.96,
			onPress = function()
				onPill(i)
			end,
		}, {
			selected and ui.box({
				key = "active-pill",
				matched = "showcase-pill",
				matchedCrop = true,
				pin = { left = 0, top = 0, right = 0, bottom = 0 },
				radius = 6,
				fill = ui.hex("#009fff"),
			}),
			ui.text(label, {
				size = 12,
				weight = "medium",
				color = if selected then ui.hex("#ffffff") else "text",
			}),
		})
	end

	local flying = ui.box({
		key = "flying-card",
		matched = "showcase-card",
		w = "100%",
		h = 36,
		radius = 6,
		fill = ui.hex("#009fff"),
		center = true,
		pressScale = 0.95,
		onPress = onSwap,
	}, { ui.text("Click to fly", { size = 12, weight = "semibold", color = ui.hex("#ffffff") }) })

	return ui.col({ key = "showcase", w = "full", grow = 1, pad = 16, gap = 14, scrollY = true }, {
		-- 1. Equal-column grid (`cols = 3`) with maxLines, transforms/outline, and clip/aspect/raise
		ui.box({ key = "props-grid", w = "full", cols = 3, gap = 12 }, {
			ui.col(CARD, { key = "card-clamp" }, {
				ui.text("maxLines = 2", { size = 12, weight = "semibold" }),
				ui.text(SAMPLE_TEXT, { key = "clamped-copy", size = 12, color = "text-secondary", maxLines = 2 }),
				ui.text("maxLines = 0 shows every line", { size = 11, color = "text-secondary", maxLines = 0 }),
			}),
			ui.col(CARD, { key = "card-transform", outline = ui.hex("#009fff"), outlineWidth = 2 }, {
				ui.text("rotate / scale / outline", { size = 12, weight = "semibold" }),
				ui.row({ w = "full", h = 68, center = true }, {
					ui.box({
						key = "tilted-tile",
						w = 96,
						h = 42,
						radius = 6,
						fill = "control-off",
						outline = ui.hex("#009fff"),
						outlineWidth = 1.5,
						rotate = -6,
						scale = 0.94,
						origin = { x = 0.5, y = 0.5 },
						offset = { x = 4, y = -2 },
						pressScale = 0.95,
						center = true,
						onPress = onSwap,
					}, { ui.text("-6 deg", { size = 11, mono = true }) }),
				}),
			}),
			ui.col(CARD, { key = "card-clip" }, {
				ui.text("clip = 14 / aspect / raise", { size = 12, weight = "semibold" }),
				ui.row({ w = "full", gap = 8, align = "center" }, {
					ui.box({ key = "clipped-box", w = 88, aspect = 1.6, clip = 14, fill = "control-off" }, {
						ui.box({ w = 120, h = 24, fill = ui.hex("#009fff") }),
					}),
					ui.box({
						key = "raised-badge",
						px = 8,
						py = 4,
						radius = 4,
						fill = ui.hex("#009fff"),
						raise = true,
						offset = { x = -18, y = -8 },
						role = "tooltip",
						label = "Raised badge",
					}, { ui.text("raise", { size = 11, color = ui.hex("#ffffff") }) }),
				}),
			}),
		}),

		-- 2. Shared-element flight (`matched` + `matchedCrop`) and `role = "tablist"`
		ui.col(CARD, { key = "card-matched", w = "full" }, {
			ui.row({ w = "full", align = "center", justify = "between" }, {
				ui.text("matched + matchedCrop", { size = 12, weight = "semibold" }),
				ui.row({ key = "demo-tablist", role = "tablist", label = "Showcase tabs", gap = 4, pad = 3, radius = 8, fill = "control-off" }, tabChildren),
			}),
			ui.box({ w = "full", cols = 2, gap = 12 }, {
				ui.box({ key = "slot-0", h = 44, pad = 4, radius = 8, fill = "bg" }, { cardSlot == 0 and flying }),
				ui.box({ key = "slot-1", h = 44, pad = 4, radius = 8, fill = "bg" }, { cardSlot == 1 and flying }),
			}),
		}),

		-- 3. Percentage sizing (`w = "40%"`, `h = "60%"`) and ARIA roles
		ui.col(CARD, { key = "card-percent", w = "full" }, {
			ui.text("Percentage sizes (40% / 75% width, 60% height) & ARIA roles", { size = 12, weight = "semibold" }),
			ui.box({ key = "pct-track", w = "full", h = 40, pad = 4, radius = 6, fill = "bg", role = "progressbar", label = "40 percent bar" }, {
				ui.box({ key = "pct-fill", w = "40%", h = "60%", radius = 4, fill = ui.hex("#009fff") }),
			}),
			ui.row({ w = "full", gap = 8 }, {
				ui.box({ key = "role-slider", role = "slider", label = "Gain", w = "75%", h = 10, radius = 5, fill = "control-off" }),
				ui.text("75%", { size = 11, mono = true, color = "text-secondary" }),
			}),
		}),

		-- 4. Coalesced `onScroll` indicator and `view.reducedMotion` readout
		ui.col(CARD, { key = "card-scroll-motion", w = "full" }, {
			ui.row({ w = "full", align = "center", justify = "between", gap = 8 }, {
				ui.text("onScroll & view.reducedMotion", { size = 12, weight = "semibold" }),
				ui.row({ gap = 8, align = "center" }, {
					ui.box({
						key = "reduced-motion-readout",
						role = "status",
						label = if reducedMotion then "reducedMotion on" else "reducedMotion off",
						px = 8,
						py = 2,
						radius = 4,
						fill = "control-off",
					}, {
						ui.text(if reducedMotion then "reducedMotion: on" else "reducedMotion: off", { size = 11, mono = true }),
					}),
					ui.text(
						`{math.round(scrollY)} / {math.round(scrollMaxY)} · {math.round(scrollFy * 100)}%`,
						{ key = "scroll-readout", role = "status", size = 11, mono = true, tabular = true, color = "text-secondary" }
					),
				}),
			}),
			ui.box({ key = "scroll-progress", w = "full", h = 6, radius = 3, fill = "bg", role = "progressbar", label = "Scroll progress" }, {
				ui.box({ w = `{math.round(scrollFy * 100)}%`, h = "100%", radius = 3, fill = ui.hex("#009fff") }),
			}),
			ui.col({
				key = "scroll-demo",
				label = "Scrollable demo list",
				w = "full",
				h = 96,
				pad = 6,
				gap = 4,
				radius = 6,
				fill = "bg",
				scrollY = true,
				onScroll = onScroll,
			}, {
				ui.text("Scroll line 1 — coalesced onScroll delivers x, y, maxX, maxY, fy", { size = 11, color = "text-secondary" }),
				ui.text("Scroll line 2 — unchanged re-renders never re-fire", { size = 11, color = "text-secondary" }),
				ui.text("Scroll line 3 — multiple moves before poll keep newest offset", { size = 11, color = "text-secondary" }),
				ui.text("Scroll line 4 — fy clamps to [0, 1] and is 0 when maxY == 0", { size = 11, color = "text-secondary" }),
				ui.text("Scroll line 5 — watch the progress bar and readout above", { size = 11, color = "text-secondary" }),
				ui.text("Scroll line 6 — works on ui.col, ui.row, ui.box, and ui.virtualList", { size = 11, color = "text-secondary" }),
				ui.text("Scroll line 7 — view.reducedMotion follows system or cx override", { size = 11, color = "text-secondary" }),
				ui.text("Scroll line 8 — bottom of scrollable demo list", { size = 11, color = "text-secondary" }),
			}),
		}),
	})
end

return {
	render = render,
}
