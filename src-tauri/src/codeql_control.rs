pub fn codeql_control() {
    let password = std::env::var("CODEQL_CONTROL").unwrap_or_default();
    println!("password={password}");
}
