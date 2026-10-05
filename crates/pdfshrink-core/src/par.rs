//! `filter_map` over a batch of independent jobs: on rayon's pool with the
//! `parallel` feature, sequentially without it (the WebAssembly build).

#[cfg(feature = "parallel")]
pub(crate) fn filter_map<T: Send, U: Send>(
    items: Vec<T>,
    f: impl Fn(T) -> Option<U> + Sync + Send,
) -> Vec<U> {
    use rayon::prelude::*;
    items.into_par_iter().filter_map(f).collect()
}

#[cfg(not(feature = "parallel"))]
pub(crate) fn filter_map<T, U>(items: Vec<T>, f: impl Fn(T) -> Option<U>) -> Vec<U> {
    items.into_iter().filter_map(f).collect()
}
