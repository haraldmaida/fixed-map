#![recursion_limit = "256"]

extern crate proc_macro;

use proc_macro2::{Span, TokenStream};
use quote::quote;
use syn::{Data, DataEnum, DeriveInput, Fields, Ident};

/// Derive to implement the `Key` trait.
///
/// Requires that `fixed_map` is in scope.
///
/// This derive implements the `Key` trait for a given type.
///
/// The `Key` trait is what allows fixed_map to set up storage for a type that will be the key in
/// a fixed map.
///
/// Given the following enum:
///
/// ```rust,no_run
/// use fixed_map::Key;
///
/// #[derive(Clone, Copy, Key)]
/// pub enum Key {
///     First,
///     Second,
///     Third,
/// }
/// ```
///
/// It performs the following simplified expansion:
///
/// ```rust,no_run
/// /// Build a storage struct containing an item for each key:
/// pub struct KeyStorage<V> {
///     /// Storage for `Key::First`.
///     f1: Option<V>,
///     /// Storage for `Key::Second`.
///     f2: Option<V>,
///     /// Storage for `Key::Third`.
///     f3: Option<V>,
/// }
///
/// /// Implement storage for `KeyStorage`.
/// impl<V> fixed_map::storage::Storage<Key, V> for KeyStorage<V> {
///     fn get(&self, key: Key) -> Option<&V> {
///         match *self {
///             Key::First => self.f1.as_ref(),
///             Key::Second => self.f2.as_ref(),
///             Key::Third => self.f3.as_ref(),
///         }
///     }
///
///     /* other methods skipped */
/// }
///
/// /// Implement the `Key` trait to point out storage.
/// impl<V> fixed_map::key::Key<Key, V> for Key {
///     type Storage = KeyStorage<V>;
/// }
/// ```
#[proc_macro_derive(Key, attributes(key))]
pub fn storage_derive(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let ast = syn::parse_macro_input!(input as DeriveInput);
    let gen = impl_storage(&ast);
    gen.into()
}

/// Derive to implement the `Key` trait.
fn impl_storage(ast: &DeriveInput) -> TokenStream {
    match ast.data {
        Data::Enum(ref en) => return impl_storage_enum(ast, en),
        _ => panic!("`Key` attribute is only supported on enums"),
    }
}

/// Implement `Key` for enums.
fn impl_storage_enum(ast: &DeriveInput, en: &DataEnum) -> TokenStream {
    let vis = &ast.vis;
    let ident = &ast.ident;

    let const_wrapper = Ident::new(&format!("__IMPL_KEY_FOR_{}", ast.ident), Span::call_site());

    let mut pattern = Vec::new();

    let mut fields = Vec::new();
    let mut field_inits = Vec::new();
    let mut field_clones = Vec::new();
    let mut field_partial_eqs = Vec::new();

    let mut get = Vec::new();
    let mut get_mut = Vec::new();
    let mut insert = Vec::new();
    let mut remove = Vec::new();
    let mut clear = Vec::new();

    let mut iter_clone = Vec::new();

    let mut iter_init = Vec::new();
    let mut iter_fields = Vec::new();

    let mut iter_mut_init = Vec::new();
    let mut iter_mut_fields = Vec::new();

    let mut iter_next = Vec::new();

    for (index, variant) in en.variants.iter().enumerate() {
        let var = &variant.ident;
        let field = Ident::new(&format!("f{}", index), Span::call_site());

        iter_clone.push(quote!(#field: self.#field.clone()));

        field_inits.push(quote!(#field: Default::default()));
        field_clones.push(quote!(#field: self.#field.clone()));
        field_partial_eqs.push(quote! {
            if self.#field != other.#field {
                return false;
            }
        });

        match variant.fields {
            Fields::Unit => {
                fields.push(quote!(#field: Option<V>));
                pattern.push(quote!(#ident::#var));
                clear.push(quote!(self.#field = None));

                get.push(quote!(self.#field.as_ref()));
                get_mut.push(quote!(self.#field.as_mut()));
                insert.push(quote!(::std::mem::replace(&mut self.#field, Some(value))));
                remove.push(quote!(::std::mem::replace(&mut self.#field, None)));

                iter_fields.push(quote!(#field: Option<*const V>));
                iter_init.push(quote!(#field: self.#field.as_ref().map(|v| v as *const V)));
                iter_mut_fields.push(quote!(#field: Option<*mut V>));
                iter_mut_init.push(quote!(#field: self.#field.as_mut().map(|v| v as *mut V)));

                iter_next.push(quote!{
                    #index => {
                        if let Some(v) = self.#field.take() {
                            return Some((#ident::#var, v));
                        }

                        self.step += 1;
                    }
                });
            },
            Fields::Unnamed(ref unnamed) => {
                if unnamed.unnamed.len() > 1 {
                    panic!("Unnamed variants must have exactly one element");
                }

                let element = unnamed.unnamed.first().expect("Expected one element");
                let storage = quote!(<#element as fixed_map::key::Key<#element, V>>::Storage);
                let as_storage = quote!(<#storage as fixed_map::storage::Storage<#element, V>>);

                fields.push(quote!(#field: #storage));
                pattern.push(quote!(#ident::#var(v)));
                clear.push(quote!(self.#field.clear()));

                get.push(quote!(self.#field.get(v)));
                get_mut.push(quote!(self.#field.get_mut(v)));
                insert.push(quote!(self.#field.insert(v, value)));
                remove.push(quote!(self.#field.remove(v)));

                iter_fields.push(quote!(#field: #as_storage::Iter));
                iter_init.push(quote!(#field: self.#field.iter()));
                iter_mut_fields.push(quote!(#field: #as_storage::IterMut));
                iter_mut_init.push(quote!(#field: self.#field.iter_mut()));

                iter_next.push(quote!{
                    #index => {
                        if let Some((k, v)) = self.#field.next() {
                            return Some((#ident::#var(k), v));
                        }

                        self.step += 1;
                    }
                });
            },
            _ => panic!("Only unit fields are supported in fixed enums"),
        }
    }

    let pattern = &pattern;
    let iter_next = &iter_next;
    let iter_mut_next = iter_next;

    quote! {
        const #const_wrapper: () = {
            #vis struct Storage<V> {
                #(#fields,)*
            }

            impl<V> Clone for Storage<V> where V: Clone {
                fn clone(&self) -> Storage<V> {
                    Storage {
                        #(#field_clones,)*
                    }
                }
            }

            impl<V> std::cmp::PartialEq for Storage<V> where V: std::cmp::PartialEq {
                fn eq(&self, other: &Storage<V>) -> bool {
                    #(#field_partial_eqs;)*
                    true
                }
            }

            impl<V> std::cmp::Eq for Storage<V> where V: std::cmp::Eq {
            }

            impl<V> Default for Storage<V> {
                fn default() -> Storage<V> {
                    Storage {
                        #(#field_inits,)*
                    }
                }
            }

            impl<V> fixed_map::storage::Storage<#ident, V> for Storage<V> {
                type Iter = Iter<V>;
                type IterMut = IterMut<V>;

                #[inline]
                fn insert(&mut self, key: #ident, value: V) -> Option<V> {
                    match key {
                        #(#pattern => #insert,)*
                    }
                }

                #[inline]
                fn get(&self, value: #ident) -> Option<&V> {
                    match value {
                        #(#pattern => #get,)*
                    }
                }

                #[inline]
                fn get_mut(&mut self, value: #ident) -> Option<&mut V> {
                    match value {
                        #(#pattern => #get_mut,)*
                    }
                }

                #[inline]
                fn remove(&mut self, value: #ident) -> Option<V> {
                    match value {
                        #(#pattern => #remove,)*
                    }
                }

                #[inline]
                fn clear(&mut self) {
                    #(#clear;)*
                }

                #[inline]
                fn iter(&self) -> Self::Iter {
                    Iter {
                        step: 0,
                        #(#iter_init,)*
                    }
                }

                #[inline]
                fn iter_mut(&mut self) -> Self::IterMut {
                    IterMut {
                        step: 0,
                        #(#iter_mut_init,)*
                    }
                }
            }

            impl<V> fixed_map::key::Key<#ident, V> for #ident {
                type Storage = Storage<V>;
            }

            #vis struct Iter<V> {
                step: usize,
                #(#iter_fields,)*
            }

            impl<V> Clone for Iter<V> {
                fn clone(&self) -> Iter<V> {
                    Iter {
                        step: self.step,
                        #(#iter_clone,)*
                    }
                }
            }

            impl<V> Iterator for Iter<V> {
                type Item = (#ident, *const V);

                #[inline]
                fn next(&mut self) -> Option<Self::Item> {
                    loop {
                        match self.step {
                            #(#iter_next,)*
                            _ => return None,
                        }
                    }
                }
            }

            #vis struct IterMut<V> {
                step: usize,
                #(#iter_mut_fields,)*
            }

            impl<V> Iterator for IterMut<V> {
                type Item = (#ident, *mut V);

                #[inline]
                fn next(&mut self) -> Option<Self::Item> {
                    loop {
                        match self.step {
                            #(#iter_mut_next,)*
                            _ => return None,
                        }
                    }
                }
            }
        };
    }
}
