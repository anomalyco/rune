"""Retargets split trajectories, never excuse bad motion between destinations."""
import json
import os
import unittest

import analyze
from test_analyze import scene


class Retargets(unittest.TestCase):
    def findings(self, xs, goals=None, parent=None):
        frames = [{"root": (0, 0, 400, 400, 1), "root/piece": (x, 40, 10, 10, 1)} for x in xs]
        frames = [frames[0]] * 80 + frames + [frames[-1]] * 80
        path = scene(frames)
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


if __name__ == "__main__":
    unittest.main()
