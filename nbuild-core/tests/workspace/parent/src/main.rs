fn main() {
    let answer = child::one();
    let version = child::version();
    println!("Hello, {answer}");
    println!("I'm was compiled with version {version}");
}
