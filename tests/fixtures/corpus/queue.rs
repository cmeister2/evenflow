//! Deliberately missing explanations at three independent production boundaries.

fn process_queue() {
    for item in items() {
        let decoded = decode(item);
        cache(&decoded);
        deliver(decoded);
    }
}

fn recover() {
    match result() {
        Ok(value) => {
            let normalized = normalize(value);
            persist(&normalized);
            acknowledge(normalized);
        }
        Err(error) => {
            // Record recovery before releasing the lease so another worker can resume.
            record(&error);
            release_lease();
            schedule_retry(error);
        }
    }
}

fn nested_ownership() {
    if ready() {
        // Preparation and finalization surround both the initial attempt and its retry.
        prepare();
        if should_retry() {
            fetch();
            validate();
            retry();
        }
        finish();
    }
}
