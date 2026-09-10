fn main() -> Result<(), Box<dyn std::error::Error>> {
    himark_winit::run(himark_winit::Options::from_env())
}
