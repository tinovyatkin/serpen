fn main() {
    let content = "hello\r\nworld\r\ntest\r\n";
    let normalized = content.replace("\r\n", "\n");
    println!("Original: {:?}", content);
    println!("Normalized: {:?}", normalized);
}