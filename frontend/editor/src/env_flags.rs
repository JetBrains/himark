macro_rules! flag {
    ($name:ident, $var:literal) => {
        pub(crate) fn $name() -> bool {
            static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
            *ENABLED.get_or_init(|| std::env::var_os($var).is_some())
        }
    };
}

flag!(paint_probe, "HIMARK_PAINT_PROBE");
flag!(trace_diff, "HIMARK_TRACE_DIFF");
flag!(landing_probe, "HIMARK_LANDING_PROBE");
flag!(trace_repair, "HIMARK_TRACE_REPAIR");
flag!(trace_resize, "HIMARK_TRACE_RESIZE");
flag!(trace_skip, "HIMARK_TRACE_SKIP");
