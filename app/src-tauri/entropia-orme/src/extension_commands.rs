// The commands an extension adds to the application command surface, by
// path from the crate root. A callback macro of the same shape as
// `app_command_surface!`, which hands it the shared lists to complete. The
// default: none.
macro_rules! extension_command_surface {
    ($shape:ident, $($surface:tt)*) => {
        $shape! {
            $($surface)*
            extension: [],
        }
    };
}
