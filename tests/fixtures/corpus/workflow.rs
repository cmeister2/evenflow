//! Representative production-style code whose structural explanations are complete.

fn save_if_dirty(state: &mut State) {
    if state.is_dirty() {
        // Keep persistence ahead of the clean marker so a failed write can be retried.
        let snapshot = state.snapshot();
        persist(snapshot);
        state.mark_clean();
    }
}

fn gather_results() {
    for input in inputs() {
        // Validate the decoded payload before inserting it into the shared cache.
        let value = decode(input);
        validate(&value);
        cache(value);
    }
}

fn field_copies(destination: &mut State, source: &State) {
    if destination.is_empty() {
        destination.first = source.first;
        destination.second = source.second;
        destination.label = "copied";
    }
}

fn log_only() {
    if debug_enabled() {
        tracing::trace!("starting the operation");
        log::debug!("loading cached state");
        slog::info!(logger(), "operation started");
    }
}

fn short_and_expression_only() {
    let extract = |item| item.value;
    match state() {
        Ready(value) => consume(extract(value)),
        Pending => {
            request_work();
            wait();
        }
    }
}

const fn compile_time(value: usize) -> usize {
    if value > 0 {
        let doubled = value * 2;
        let adjusted = doubled + 1;
        adjusted
    } else {
        0
    }
}

static CONFIG: LazyLock<State> = LazyLock::new(|| {
    let first = load_first();
    let second = load_second();
    State::new(first, second)
});
