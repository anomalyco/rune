use super::Trak;

/// Part of the macOS system font's track-0 row.
fn system() -> Trak {
    let sizes = [(12.0, 0.0), (13.0, -12.0), (14.0, -22.0), (24.0, 6.0), (28.0, 28.0), (32.0, 26.0), (36.0, 21.0)];
    Trak { sizes: sizes.to_vec(), units_per_em: 2048.0 }
}

#[test]
fn spacing_moves_tracking_from_the_pixel_size_to_the_point_size() {
    let t = system();
    // 14 pt at 2x: HarfRust tracked 28 px by +28 units; 14 pt wants -22.
    assert!((t.spacing(28.0, 14.0, true) - (-22.0 - 28.0) * 28.0 / 2048.0).abs() < 1e-5);
    // At 1x, listed sizes need nothing.
    assert_eq!(t.spacing(14.0, 14.0, true), 0.0);
    // Off removes what HarfRust applied.
    assert!((t.spacing(14.0, 14.0, false) - 22.0 * 14.0 / 2048.0).abs() < 1e-5);
    // Between listed sizes Core Text truncates and HarfRust rounds: 33 pt is 24.75 units.
    assert!((t.spacing(33.0, 33.0, true) - (24.0 - 25.0) * 33.0 / 2048.0).abs() < 1e-5);
    // An explicit optical size of 14 pt at 28 pt, 2x: 14 pt's -22 units, at the real size.
    assert!((t.spacing(56.0, 14.0, true) - (-22.0 - t.units(56.0).round()) * 56.0 / 2048.0).abs() < 1e-5);
    // Beyond the table the ends hold.
    assert_eq!(t.units(8.0), 0.0);
    assert_eq!(t.units(500.0), 21.0);
}

/// A `trak` table with one horizontal track per `(track, values)` at `sizes`.
fn table(tracks: &[(f32, &[i16])], sizes: &[f32]) -> Vec<u8> {
    let fixed = |v: f32| ((v * 65536.0) as i32).to_be_bytes();
    let size_table = 12 + 8 + tracks.len() * 8;
    let values = size_table + sizes.len() * 4;
    let mut t = vec![0, 1, 0, 0, 0, 0, 0, 12, 0, 0, 0, 0];
    t.extend((tracks.len() as u16).to_be_bytes());
    t.extend((sizes.len() as u16).to_be_bytes());
    t.extend((size_table as u32).to_be_bytes());
    for (i, (track, _)) in tracks.iter().enumerate() {
        t.extend(fixed(*track));
        t.extend([0, 0]);
        t.extend(((values + i * sizes.len() * 2) as u16).to_be_bytes());
    }
    t.extend(sizes.iter().flat_map(|&s| fixed(s)));
    t.extend(tracks.iter().flat_map(|(_, v)| v.iter().flat_map(|v| v.to_be_bytes())));
    t
}

fn put(t: &mut [u8], at: usize, bytes: &[u8]) {
    t[at..at + bytes.len()].copy_from_slice(bytes);
}

/// Tracking that can't go wrong: finite, and 0 when there's no table.
fn sane(t: Option<Trak>) -> f32 {
    let spacing = t.map_or(0.0, |t| t.spacing(28.0, 14.0, true) + t.spacing(9.0, 9.0, false));
    assert!(spacing.is_finite(), "{spacing}");
    spacing
}

#[test]
fn parse_reads_track_zero_or_interpolates_around_it() {
    let sizes = [12.0, 24.0];
    let t = Trak::parse(&table(&[(-1.0, &[-40, -20]), (0.0, &[0, 10]), (1.0, &[40, 20])], &sizes), 2048).unwrap();
    assert_eq!(t.sizes, [(12.0, 0.0), (24.0, 10.0)]);
    let t = Trak::parse(&table(&[(-1.0, &[-40, -20]), (1.0, &[40, 60])], &sizes), 2048).unwrap();
    assert_eq!(t.sizes, [(12.0, 0.0), (24.0, 20.0)]);
    let t = Trak::parse(&table(&[(0.5, &[8, 16])], &sizes), 2048).unwrap();
    assert_eq!(t.sizes, [(12.0, 8.0), (24.0, 16.0)]);
}

#[test]
fn malformed_tables_track_nothing_without_panicking() {
    let good = table(&[(-1.0, &[-40, -20]), (0.0, &[0, 10]), (1.0, &[40, 20])], &[12.0, 24.0]);
    assert!(Trak::parse(&good, 0).is_none(), "zero units per em");
    for len in 0..good.len() {
        sane(Trak::parse(&good[..len], 2048));
    }
    let mut huge_offsets = good.clone();
    put(&mut huge_offsets, 6, &[0xff, 0xff]);
    assert!(Trak::parse(&huge_offsets, 2048).is_none());
    let mut huge_size_table = good.clone();
    put(&mut huge_size_table, 16, &[0xff; 4]);
    assert!(Trak::parse(&huge_size_table, 2048).is_none());
    let mut huge_counts = good.clone();
    put(&mut huge_counts, 12, &[0xff; 4]);
    assert!(Trak::parse(&huge_counts, 2048).is_none());
    // Track 0's values out of bounds: it tracks by 0, as in HarfRust.
    let mut bad_values = good.clone();
    put(&mut bad_values, 20 + 8 + 6, &[0xff, 0xff]);
    assert_eq!(Trak::parse(&bad_values, 2048).unwrap().sizes, [(12.0, 0.0), (24.0, 0.0)]);
    // A bad offset on a track that isn't used doesn't spoil the table.
    let mut bad_unused = good.clone();
    put(&mut bad_unused, 20 + 6, &[0xff, 0xff]);
    assert_eq!(Trak::parse(&bad_unused, 2048).unwrap().sizes, [(12.0, 0.0), (24.0, 10.0)]);
    for at in 0..good.len() {
        let mut t = good.clone();
        t[at] = 0xff;
        sane(Trak::parse(&t, 2048));
    }
}
