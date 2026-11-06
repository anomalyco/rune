"""Retargets split trajectories, never excuse bad motion between destinations."""
import json
import os
import unittest

import analyze
from test_analyze import scene


class Retargets(unittest.TestCase):
    def findings(self, xs, goals=None, parent=None, opacity=1, flags=None):
        frames = [{"root": (0, 0, 400, 400, 1), "root/piece": (x, 40, 10, 10, opacity)} for x in xs]
        frames = [frames[0]] * 80 + frames + [frames[-1]] * 80
        path = scene(frames, flags=flags)
        try:
            with open(path) as f:
                rows = [json.loads(line) for line in f]
            for k, mask in (goals or {}).items():
                rows[81 + k]["retarget"] = [[1 if parent else 2, mask, mask if parent == "scale" else mask & 3]]
            with open(path, "w") as f:
                f.write("\n".join(json.dumps(row) for row in rows))
            return {f["kind"] for f in analyze.analyze(path)["findings"]}
        finally:
            os.unlink(path)

    def test_a_new_destination_is_not_one_out_and_back(self):
        xs = [40, 42, 44, 46, 48, 50, 48, 46, 44, 42, 40]
        self.assertIn("reversal", self.findings(xs))
        self.assertNotIn("reversal", self.findings(xs, {6: 1}))
        # A different property's goal cannot excuse this axis.
        self.assertIn("reversal", self.findings(xs, {6: 2}))

    def test_parent_motion_carries_children_but_size_does_not(self):
        xs = [40, 42, 44, 46, 48, 50, 48, 46, 44, 42, 40]
        self.assertNotIn("reversal", self.findings(xs, {6: 15}, parent="scale"))
        self.assertIn("reversal", self.findings(xs, {6: 4}, parent="size"))

    def test_retarget_preserves_decelerating_incoming_velocity(self):
        xs = [40, 44, 48, 51, 53, 54, 53, 51, 48, 44, 40, 36, 33, 31, 30]
        self.assertNotIn("reversal", self.findings(xs, {3: 1}))
        xs[4:6] = [55, 60]  # Accelerates away instead of braking.
        self.assertIn("reversal", self.findings(xs, {3: 1}))

    def test_bad_motion_before_another_retarget_is_still_checked(self):
        xs = [40, 44, 48, 52, 50, 48, 46, 44, 42, 40, 38, 36, 34, 32]
        self.assertIn("reversal", self.findings(xs, {10: 1}))

    def test_a_jump_on_the_retarget_frame_still_fails(self):
        self.assertIn("jump", self.findings([40, 40, 40, 100, 100, 100], {3: 1}))

    def test_a_retarget_that_loses_velocity_still_fails(self):
        self.assertIn("stall", self.findings([40, 43, 46, 46.2, 49.2, 52.2, 55.2], {3: 1}))

    def thumb(self, ys, goal=None, flags=0):
        """A scroll bar's thumb beside a scrolled list, with `goal` = (frame, name) retargeted."""
        def at(y):
            return {"area": (0, 0, 400, 400, 1), "area/view": (0, 0, 390, 400, 1), "area/view/list": (0, -50, 390, 900, 1, (0, 50)),
                    "area/bar": (390, 0, 10, 400, 1), "area/bar/thumb": (392, y, 6, 120, 1), "other": (0, 0, 10, 10, 1)}
        frames = [at(ys[0])] * 80 + [at(y) for y in ys] + [at(ys[-1])] * 80
        path = scene(frames, flags={"area/bar/thumb": flags})
        try:
            with open(path) as f:
                rows = [json.loads(line) for line in f]
            ids = {m["nm"]: m["id"] for m in rows[1]["m"]}
            if goal:
                rows[81 + goal[0]]["retarget"] = [[ids[goal[1]], 8, 0]]
            with open(path, "w") as f:
                f.write("\n".join(json.dumps(row) for row in rows))
            return {f["kind"] for f in analyze.analyze(path)["findings"]}
        finally:
            os.unlink(path)

    def test_a_thumb_brakes_when_the_list_it_reads_retargets(self):
        ys = [40, 44, 48, 51, 53, 54, 53, 51, 48, 44, 40, 36, 33, 31, 30]
        self.assertIn("reversal", self.thumb(ys, flags=1))
        self.assertTrue({"reversal", "dog-leg"}.isdisjoint(self.thumb(ys, (3, "area/view/list"), flags=1)))
        # Only the content it reads moves its destination.
        self.assertIn("reversal", self.thumb(ys, (3, "other"), flags=1))

    def test_a_placed_readout_has_no_shape_of_its_own(self):
        ys = [40, 44, 48, 51, 53, 54, 53, 51, 48, 44, 40, 36, 33, 31, 30]
        self.assertNotIn("reversal", self.thumb(ys, flags=3 << 8))
        # Springing, it answers for its own path.
        self.assertIn("reversal", self.thumb(ys, flags=3 << 8 | 1))

    def test_invisible_or_intentionally_instant_retargets_are_not_stalls(self):
        xs = [40, 43, 46, 46.2, 49.2, 52.2, 55.2]
        self.assertNotIn("stall", self.findings(xs, {3: 1}, opacity=0))
        self.assertNotIn("stall", self.findings(xs, {3: 1}, flags={"root/piece": 1 << 8}))


if __name__ == "__main__":
    unittest.main()
