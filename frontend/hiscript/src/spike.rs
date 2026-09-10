fn main() {
    match hiscript::eval_to_string("`hello from quickjs ${6 * 7}`") {
        Ok(value) => println!("SPIKE OK: {value}"),
        Err(error) => {
            eprintln!("SPIKE FAILED: {error}");
            std::process::exit(1);
        }
    }
}
