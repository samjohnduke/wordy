//! The one bundled serif (Libertinus, OFL). Used by the editor, the PDF
//! template and the snippet renderer so all three agree.

pub const FAMILY: &str = "Libertinus Serif";

pub const REGULAR: &[u8] = include_bytes!("../../../assets/fonts/LibertinusSerif-Regular.otf");
pub const ITALIC: &[u8] = include_bytes!("../../../assets/fonts/LibertinusSerif-Italic.otf");
pub const BOLD: &[u8] = include_bytes!("../../../assets/fonts/LibertinusSerif-Bold.otf");
pub const BOLD_ITALIC: &[u8] =
    include_bytes!("../../../assets/fonts/LibertinusSerif-BoldItalic.otf");

pub const ALL: [&[u8]; 4] = [REGULAR, ITALIC, BOLD, BOLD_ITALIC];
