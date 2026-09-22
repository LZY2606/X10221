fn main() {
    let addr = flowline::parse_addr(std::env::args().collect());
    flowline::serve(addr, "flowline.db".into());
}
