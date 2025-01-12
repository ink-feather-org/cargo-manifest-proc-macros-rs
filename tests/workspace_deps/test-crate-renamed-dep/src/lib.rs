#[test]
pub fn test_macro_expansion() {
  let resolved_name = my_cool_dep_renamed::resolved_name!();
  assert_eq!("::my_cool_dep_renamed", resolved_name);
}
