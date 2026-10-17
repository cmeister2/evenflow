//! Macro fixtures separate visible coverage gaps from known opaque macro classes.

fn driver() {
    custom! {
        do_work();
    }
    tokio::select! {
        result = task() => {
            handle(result);
        }
    }
    custom!("braces in strings { do not establish a token group }");
    log::info!("starting", {
        build_metadata();
        metadata()
    });
    let values = vec![{
        build();
        value()
    }];
    let payload = serde_json::json!({"values": values});
    consume(payload);
}
