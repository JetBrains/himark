fn main() {
    let out = std::env::args()
        .nth(1)
        .expect("usage: grammar-pack <out-dir>");
    let out = std::path::PathBuf::from(out);
    std::fs::create_dir_all(&out).expect("out dir");

    let mut languages = editor::SyntaxLanguages::new();
    hibash::register(&mut languages);
    hic::register(&mut languages);
    hicmake::register(&mut languages);
    hicpp::register(&mut languages);
    hicsharp::register(&mut languages);
    hicss::register(&mut languages);
    hid::register(&mut languages);
    hidart::register(&mut languages);
    hielixir::register(&mut languages);
    hielm::register(&mut languages);
    hierlang::register(&mut languages);
    hifortran::register(&mut languages);
    hifsharp::register(&mut languages);
    higleam::register(&mut languages);
    higlsl::register(&mut languages);
    higo::register(&mut languages);
    higraphql::register(&mut languages);
    higroovy::register(&mut languages);
    hihaskell::register(&mut languages);
    hihcl::register(&mut languages);
    hihtml::register(&mut languages);
    hijava::register(&mut languages);
    hijavascript::register(&mut languages);
    hijson::register(&mut languages);
    hijulia::register(&mut languages);
    hikotlin::register(&mut languages);
    hilua::register(&mut languages);
    himake::register(&mut languages);
    hinix::register(&mut languages);
    hiobjc::register(&mut languages);
    hiocaml::register(&mut languages);
    hiodin::register(&mut languages);
    hiperl::register(&mut languages);
    hiphp::register(&mut languages);
    hipowershell::register(&mut languages);
    hipython::register(&mut languages);
    hir::register(&mut languages);
    hiruby::register(&mut languages);
    hirust::register(&mut languages);
    hiscala::register(&mut languages);
    hisolidity::register(&mut languages);
    hisql::register(&mut languages);
    hiswift::register(&mut languages);
    hitoml::register(&mut languages);
    hitypescript::register(&mut languages);
    hixml::register(&mut languages);
    hiyaml::register(&mut languages);
    hizig::register(&mut languages);

    if std::env::var_os("HIMARK_TIME_ENSURE").is_some() {
        let started = std::time::Instant::now();
        let mut ensured = 0;
        for entry in languages.entries() {
            if let Some(name) = entry.names().first() {
                if languages.ensure(name).is_some() {
                    ensured += 1;
                }
            }
        }
        println!(
            "ensured {ensured} languages in {:.1}ms",
            started.elapsed().as_secs_f64() * 1e3
        );
    }

    let mut manifest = String::new();
    let mut count = 0;
    for entry in languages.entries() {
        let Some(side) = entry.side() else { continue };
        std::fs::write(
            out.join(format!("{}.scm", side.module)),
            (side.highlights)(),
        )
        .expect("query written");
        manifest.push_str(&format!(
            "{}\t{}\t{}\t{}\n",
            side.module, side.symbol, side.crate_name, side.parser_dir
        ));
        count += 1;
    }
    std::fs::write(out.join("manifest.tsv"), manifest).expect("manifest written");
    println!("packed {count} grammars into {}", out.display());
}
