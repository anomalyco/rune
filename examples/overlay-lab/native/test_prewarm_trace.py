import unittest

from prewarm_trace import assert_prewarmed


class PrewarmTraceTests(unittest.TestCase):
    created = 'overlay-lab pill created OverlayCreated { window_number: Some(12), capabilities: ... }'
    acquired = 'probe acquired window=12 alpha=0.0 time=1.0'

    def test_accepts_alpha_zero_acquisition_before_creation(self):
        assert_prewarmed([self.acquired, self.created], 12)

    def test_rejects_occlusion_skip_late_or_visible_acquisition(self):
        for lines in ([self.created], [self.created, self.acquired],
                      [self.acquired.replace('alpha=0.0', 'alpha=1.0'), self.created],
                      [self.acquired.replace('window=12', 'window=11'), self.created],
                      [self.acquired]):
            with self.subTest(lines=lines), self.assertRaises(AssertionError):
                assert_prewarmed(lines, 12)


if __name__ == '__main__':
    unittest.main()
