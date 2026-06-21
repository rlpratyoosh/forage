#[macro_export]
macro_rules! info {
    ($fmt:expr $(, $arg:expr)* $(,)?) => {
        println!($fmt $(, $arg)*);
    };
}

#[macro_export]
macro_rules! error {
    ($fmt:expr $(, $arg:expr)* $(,)?) => {
        eprintln!($fmt $(, $arg)*);
    };
}
