#[cfg(test)]
pub fn pretty_macro_output(item: &proc_macro2::TokenStream) -> String {
    let Ok(file) = syn::parse_file(&item.to_string()) else {
        return item.to_string();
    };
    prettyplease::unparse(&file)
}
