use super::*;
use crate::canvas::measure;

#[test]
fn ink_reads_on_every_series_fill() {
    for dark in [false, true] {
        let paint = DiagramStyle::neutral(dark);
        for i in 0..12 {
            for fill in [solid(&paint, hue(i)), soft(&paint, hue(i))] {
                let diff = (ink(fill).to_oklab()[0] - fill.to_oklab()[0]).abs();
                assert!(diff > 0.35, "dark={dark} series {i}: {diff}");
            }
        }
    }
}

#[test]
fn axis_labels_stay_inside_and_never_overlap() {
    let paint = DiagramStyle::neutral(false);
    let mut c = Canvas::default();
    let ticks: Vec<(f32, Measured)> = [0.0, 10.0, 60.0, 200.0].iter().map(|&x| (x, measure("2024-01-01", paint.font))).collect();
    axis_x(&mut c, &ticks, [0.0, 200.0], [0.0, 100.0], paint.font, &paint);
    let xs: Vec<f32> = c.labels.iter().map(|l| l.at[0]).collect();
    assert!(c.labels.len() < 4, "the tick at 10 crowds the first: {xs:?}");
    for l in &c.labels {
        assert!(l.at[0] >= 0.0 && l.at[0] + l.size[0] <= 200.0 + 0.01, "{xs:?}");
    }
    for w in c.labels.windows(2) {
        assert!(w[1].at[0] >= w[0].at[0] + w[0].size[0], "{xs:?}");
    }
}
