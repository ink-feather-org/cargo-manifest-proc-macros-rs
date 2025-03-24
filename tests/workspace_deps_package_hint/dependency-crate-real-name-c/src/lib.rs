pub use ::dependency_crate_real_name_c_macros::*;

#[test]
pub fn test_macro_expansion() {
  let resolved_name = ::dependency_crate_real_name_c_macros::resolved_name!();
  assert_eq!("::dependency_crate_real_name_c", resolved_name);
}
