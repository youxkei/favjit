//! Replay a trace taken on real hardware (ADR-0009).
//!
//! This is the debugging entry point: point it at a trace the watchdog kept and it
//! runs the real pipeline over the real event stream, printing what came out.
//!
//! ```text
//! FAVJIT_TRACE=/path/to/trace cargo test -p favjit-e2e --test replay_a_file -- --nocapture
//! ```
//!
//! It lives in the suite rather than in the `favjit` binary because replaying needs
//! the simulated host, and ADR-0005 keeps that out of anything shipped. It also
//! keeps the command that shows keystrokes separate from the ones that run the
//! converter: a trace holds whatever was typed in the window it covers, so seeing
//! it should take asking for it by name.
//!
//! With no path set it passes without doing anything, so `cargo test --workspace`
//! is not a thing that needs a trace to hand.

use favjit_core::{sink, trace::Trace, Layout};
use favjit_host_sim::SimHost;

#[test]
fn replay_the_trace_at_favjit_trace() {
    let Ok(path) = std::env::var("FAVJIT_TRACE") else {
        println!("FAVJIT_TRACE is not set; nothing to replay");
        return;
    };
    let bytes = std::fs::read(&path).expect("read the trace");
    let trace = Trace::read(&bytes);

    println!("{path}: {} records", trace.records().count());
    println!("dropped from the start: {}", trace.evicted());
    println!("begins at a checkpoint: {}", trace.begins_at_a_checkpoint());

    let events = trace.events();
    println!("\n{} events, replaying:", events.len());
    for event in &events {
        println!("  {:>12} ns  {:?}", event.at.as_nanos(), event.kind);
    }

    let mut host = SimHost::from_trace(&trace);
    match trace.checkpoint() {
        // The loop directly, because a recording is not a machine to bring up: what
        // a replay reconstructs is the conversion, and everything a run does before
        // it happened once, on the machine the trace came from.
        Some(checkpoint) => {
            println!("\nfrom the checkpoint: {checkpoint:?}");
            sink::convert_from(Layout::dudrack(), None, &checkpoint, &mut host);
        }
        None => sink::convert(Layout::dudrack(), None, &mut host),
    }

    // What the replay produced, beside what the run did, because the two
    // disagreeing is the one thing this cannot tell you on its own: a replay is
    // only evidence while it matches, and the trace holds what was really sent.
    let replayed = host.injected();
    let recorded = trace.injected();
    println!("\n{} injections on replay:", replayed.len());
    for injected in &replayed {
        println!("  {injected:?}");
    }
    println!(
        "\nthe run itself sent {} — {}",
        recorded.len(),
        if recorded == replayed {
            "the same, so the replay is faithful".to_string()
        } else {
            format!("different, which is a bug in the pipeline or in the trace:\n  recorded: {recorded:?}")
        }
    );
}
