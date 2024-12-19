use syn::token::PathSep;

/// Converts a crate name to a global [`syn::Path`].
/// It transforms '-' to '_' and adds a leading colon.
///
/// Note: This does not resolve the crate name to a path.
#[must_use = "This function returns the path to the crate as a syn::Path"]
pub fn crate_name_to_path(crate_name: &str) -> syn::Path {
    let crate_name = crate_name.replace('-', "_");
    let mut path = syn::parse_str::<syn::Path>(crate_name.as_str())
        .expect("Failed to parse crate name as path");
    path.leading_colon = Some(PathSep::default());
    path
}

/// Pretty format a [`syn::Path`] as a string.
#[must_use = "This function returns the formatted path as a string"]
pub fn pretty_format_syn_path(path: &syn::Path) -> String {
    let mut path_str = String::new();
    let has_leading_colon = path.leading_colon.is_some();
    if has_leading_colon {
        path_str.push_str("::");
    }
    for segment in &path.segments {
        path_str.push_str(&segment.ident.to_string());
        path_str.push_str("::");
    }
    path_str.truncate(path_str.len() - 2);
    path_str
}
