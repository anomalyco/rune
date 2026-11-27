//! Validation for indirect compute dispatch arguments (`DispatchIndirectArgs`).

/// Required byte alignment for `ComputePass::dispatch_workgroups_indirect` offsets in WebGPU and
/// Metal.
pub(crate) const DISPATCH_OFFSET_ALIGNMENT: wgpu::BufferAddress = 4;

/// Byte size of `wgpu::wgt::DispatchIndirectArgs` (`[u32; 3]`, matching Metal's
/// `MTLDispatchThreadgroupsIndirectArguments`).
pub(crate) const DISPATCH_ARGS_SIZE: wgpu::BufferAddress =
    std::mem::size_of::<wgpu::wgt::DispatchIndirectArgs>() as wgpu::BufferAddress;

/// Validates an indirect compute dispatch buffer and offset before encoding Metal's
/// `dispatchThreadgroupsWithIndirectBuffer:indirectBufferOffset:threadsPerThreadgroup:`.
///
/// Returns the validated byte offset as `usize` or a WebGPU-aligned validation error message.
pub(crate) fn validate_dispatch(
    usage: wgpu::BufferUsages,
    offset: wgpu::BufferAddress,
    buffer_size: wgpu::BufferAddress,
) -> Result<usize, String> {
    if !usage.contains(wgpu::BufferUsages::INDIRECT) {
        return Err("dispatch_workgroups_indirect buffer is missing BufferUsages::INDIRECT".into());
    }
    if !offset.is_multiple_of(DISPATCH_OFFSET_ALIGNMENT) {
        return Err(format!(
            "dispatch_workgroups_indirect offset {offset} must be a multiple of {DISPATCH_OFFSET_ALIGNMENT} bytes"
        ));
    }
    let end = offset.checked_add(DISPATCH_ARGS_SIZE).ok_or_else(|| {
        format!(
            "dispatch_workgroups_indirect offset {offset} overflows {DISPATCH_ARGS_SIZE}-byte DispatchIndirectArgs"
        )
    })?;
    if end > buffer_size {
        return Err(format!(
            "dispatch_workgroups_indirect range {offset}..{end} exceeds buffer size {buffer_size}"
        ));
    }
    usize::try_from(offset).map_err(|_| {
        format!("dispatch_workgroups_indirect offset {offset} exceeds platform pointer width")
    })
}

#[cfg(test)]
#[path = "indirect_tests.rs"]
mod tests;
