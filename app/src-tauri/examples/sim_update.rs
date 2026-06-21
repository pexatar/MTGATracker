//! Strumento di TEST (non fa parte dell'app): modifica il conteggio di
//! riferimento salvato nel database per simulare l'uscita di nuove carte,
//! così da poter verificare a occhio il rilevamento automatico.
//!
//! Uso:
//!   cargo run --example sim_update -- "<percorso cards.sqlite>" <valore>
//!   cargo run --example sim_update -- "<percorso cards.sqlite>" reset

use rusqlite::Connection;

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("percorso del database mancante");
    let action = args.next().expect("indicare un valore numerico oppure 'reset'");

    let conn = Connection::open(&path).expect("impossibile aprire il database");

    if action == "reset" {
        conn.execute("DELETE FROM meta WHERE key = 'source_arena_count'", [])
            .expect("errore nel reset");
        println!("Riferimento azzerato: al prossimo avvio l'app lo reimposterà al valore reale.");
        return;
    }

    let value: i64 = action.parse().expect("il valore deve essere un numero");
    conn.execute(
        "INSERT INTO meta(key, value) VALUES('source_arena_count', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        [value.to_string()],
    )
    .expect("errore nello scrivere il riferimento");
    println!("Riferimento impostato a {value}: l'app vedrà come 'nuove' le carte oltre questo numero.");
}
