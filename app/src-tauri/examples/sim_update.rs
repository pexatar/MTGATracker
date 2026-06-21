//! TEST tool (not part of the app): changes the reference count stored in the
//! database to simulate the release of new cards, so the automatic detection
//! can be verified by eye.
//!
//! Usage:
//!   cargo run --example sim_update -- "<path to cards.sqlite>" <value>
//!   cargo run --example sim_update -- "<path to cards.sqlite>" reset

use rusqlite::Connection;

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("missing database path");
    let action = args.next().expect("provide a numeric value or 'reset'");

    let conn = Connection::open(&path).expect("could not open the database");

    if action == "reset" {
        conn.execute("DELETE FROM meta WHERE key = 'source_arena_count'", [])
            .expect("reset failed");
        println!("Reference cleared: on next launch the app will reset it to the real value.");
        return;
    }

    let value: i64 = action.parse().expect("the value must be a number");
    conn.execute(
        "INSERT INTO meta(key, value) VALUES('source_arena_count', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        [value.to_string()],
    )
    .expect("error writing the reference");
    println!("Reference set to {value}: the app will treat cards beyond this number as 'new'.");
}
