use super::{ErrorLog, Note, with_error_scopes_using};

/// An invalid pipeline set every frame logs once, not once a frame.
#[test]
fn a_repeated_error_is_logged_once() {
    let mut log = ErrorLog::default();
    let error = "[Invalid RenderPipeline \"uber\"] is invalid due to a previous error.";
    assert_eq!(log.note(error), Note::First);
    for _ in 0..300 { assert_eq!(log.note(error), Note::Quiet); }
}

/// Different errors are each logged, up to a limit, then one line says the rest are muted.
#[test]
fn different_errors_are_logged_until_the_limit() {
    let mut log = ErrorLog::default();
    let notes: Vec<Note> = (0..8).map(|n| log.note(&format!("error {n}"))).collect();
    assert_eq!(notes, [Note::First, Note::New, Note::New, Note::New, Note::Mute, Note::Quiet, Note::Quiet, Note::Quiet]);
    assert_eq!(log.note("error 0"), Note::Quiet);
}

/// wgpu-core panics with "Mismatched pop_error_scope call: error scopes must be popped in reverse
/// order" if scopes are popped out of LIFO order, and leaks open scopes if popping short-circuits.
/// Both native and wasm32 WebGL2 share `with_error_scopes_using`, so this test guards both.
#[test]
fn error_scopes_are_pushed_and_popped_in_strict_reverse_order_without_short_circuit() {
    use std::cell::RefCell;

    let stack = RefCell::new(Vec::<(u32, wgpu::ErrorFilter)>::new());
    let pushed = RefCell::new(Vec::<wgpu::ErrorFilter>::new());
    let popped = RefCell::new(Vec::<wgpu::ErrorFilter>::new());
    let next_id = RefCell::new(0u32);

    let (value, errors) = with_error_scopes_using(
        |filter| {
            let mut id = next_id.borrow_mut();
            *id += 1;
            let guard = *id;
            stack.borrow_mut().push((guard, filter));
            pushed.borrow_mut().push(filter);
            guard
        },
        || {
            assert_eq!(stack.borrow().len(), 3, "all three scopes must be active during op");
            "ran"
        },
        |guard| {
            let (top_id, filter) = stack
                .borrow_mut()
                .pop()
                .expect("pop_error_scope called on empty stack");
            assert_eq!(
                top_id, guard,
                "Mismatched pop_error_scope call: error scopes must be popped in reverse order"
            );
            popped.borrow_mut().push(filter);
            // Return Some for every scope to prove popping never short-circuits.
            Some(filter)
        },
    );

    assert_eq!(value, "ran");
    assert!(stack.borrow().is_empty(), "every pushed scope must be popped");
    assert_eq!(
        *pushed.borrow(),
        [
            wgpu::ErrorFilter::Validation,
            wgpu::ErrorFilter::OutOfMemory,
            wgpu::ErrorFilter::Internal,
        ]
    );
    assert_eq!(
        *popped.borrow(),
        [
            wgpu::ErrorFilter::Internal,
            wgpu::ErrorFilter::OutOfMemory,
            wgpu::ErrorFilter::Validation,
        ]
    );
    assert_eq!(errors.internal, Some(wgpu::ErrorFilter::Internal));
    assert_eq!(errors.memory, Some(wgpu::ErrorFilter::OutOfMemory));
    assert_eq!(errors.validation, Some(wgpu::ErrorFilter::Validation));
}
