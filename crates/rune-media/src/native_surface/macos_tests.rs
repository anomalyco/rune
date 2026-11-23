#![allow(unsafe_code)]

use super::*;

fn set_matrix(buffer: &PixelBuffer, value: Option<&CFString>) {
    // SAFETY: `buffer.0` is a uniquely owned `CVPixelBuffer` and `kCVImageBufferYCbCrMatrixKey`
    // is an immutable static `CFString` constant.
    unsafe {
        match value {
            Some(v) => buffer.0.set_attachment(
                kCVImageBufferYCbCrMatrixKey,
                v.as_ref(),
                CVAttachmentMode::ShouldPropagate,
            ),
            None => buffer.0.remove_attachment(kCVImageBufferYCbCrMatrixKey),
        }
    }
}

#[test]
fn ycbcr_matrix_5_and_6_map_to_bt601_and_other_codes_are_rejected() {
    let buffer = PixelBuffer::from_bytes(&synthetic(16, 16, 0)).unwrap();

    // Named CoreVideo constants keep their existing mappings.
    // SAFETY: both `kCVImageBufferYCbCrMatrix_*` statics are immutable CoreVideo constants.
    unsafe {
        set_matrix(&buffer, Some(kCVImageBufferYCbCrMatrix_ITU_R_601_4));
        assert_eq!(buffer.info().unwrap().2.matrix, Matrix::Bt601);
        set_matrix(&buffer, Some(kCVImageBufferYCbCrMatrix_ITU_R_709_2));
        assert_eq!(buffer.info().unwrap().2.matrix, Matrix::Bt709);
    }

    // Unnamed code-point strings for H.264 / H.273 matrix_coefficients 5 (BT.470BG) and 6 (SMPTE 170M).
    for code in ["YCbCrMatrix#5", "YCbCrMatrix#6"] {
        let tag = CFString::from_str(code);
        set_matrix(&buffer, Some(&tag));
        assert_eq!(buffer.info().unwrap().2.matrix, Matrix::Bt601, "{code}");
    }

    // Pushing a buffer tagged with "YCbCrMatrix#5" succeeds and records Matrix::Bt601 on the frame.
    let tag5 = CFString::from_str("YCbCrMatrix#5");
    set_matrix(&buffer, Some(&tag5));
    let surface = NativeSurface::new();
    surface.push(buffer.clone(), 1).unwrap();
    assert_eq!(surface.latest().unwrap().color.matrix, Matrix::Bt601);

    // Absent matrix keeps the SD/HD height convention.
    set_matrix(&buffer, None);
    assert_eq!(buffer.info().unwrap().2.matrix, Matrix::Bt601);
    let hd = PixelBuffer::from_bytes(&synthetic(16, 720, 0)).unwrap();
    set_matrix(&hd, None);
    assert_eq!(hd.info().unwrap().2.matrix, Matrix::Bt709);

    // Unsupported matrices (BT.2020, SMPTE 240M, FCC, unknown) stay rejected.
    // SAFETY: both `kCVImageBufferYCbCrMatrix_*` statics are immutable CoreVideo constants.
    unsafe {
        for value in [kCVImageBufferYCbCrMatrix_ITU_R_2020, kCVImageBufferYCbCrMatrix_SMPTE_240M_1995] {
            set_matrix(&buffer, Some(value));
            let err = buffer.info().unwrap_err();
            assert!(err.contains("unsupported YCbCr matrix"), "unexpected error: {err}");
        }
    }
    for code in ["YCbCrMatrix#4", "YCbCrMatrix#7", "YCbCrMatrix#9", "unknown"] {
        let tag = CFString::from_str(code);
        set_matrix(&buffer, Some(&tag));
        let err = buffer.info().unwrap_err();
        assert!(err.contains("unsupported YCbCr matrix"), "{code}: unexpected error: {err}");
    }
}
