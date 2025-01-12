pub use ::my_cool_dep_real_name_macros::*;

#[test]
pub fn test_macro_expansion() {
  let resolved_name = ::my_cool_dep_real_name_macros::resolved_name!();
  assert_eq!("::my_cool_dep_real_name", resolved_name);
}
