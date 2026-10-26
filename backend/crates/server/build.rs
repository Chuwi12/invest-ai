fn main() {
    // `sqlx::migrate!("./migrations")` embebe los `.sql` al compilar: sin esto, cambiar uno no recompila el crate.
    println!("cargo:rerun-if-changed=migrations");
}
