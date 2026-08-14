fn main() {
    if !matches!(
        operation_executor::run_stdio(),
        Ok(operation_executor::ServerExit::CoordinatorEof)
    ) {
        std::process::exit(1);
    }
}
