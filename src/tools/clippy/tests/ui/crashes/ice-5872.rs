#![warn(clippy::needless_collect)]

fn main() {
    let _ = vec![1, 2, 3].into_iter().collect::<Vec<_>>().is_empty();
    //~^ ERROR: avoid using `collect()` when not needed
    //~| NOTE: `-D clippy::needless-collect` implied by `-D warnings`
}
