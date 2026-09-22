use std::collections::BTreeMap;
use flowline::fcs::*;
fn main() {
    let ch=vec![FcsChannelSpec{name:"A".into(),long_name:"A".into(),range:262144},
                FcsChannelSpec{name:"B".into(),long_name:"B".into(),range:262144}];
    let b=write_fcs(&ch,&[vec![1u16,2]],&BTreeMap::new());
    println!("len={}",b.len());
    println!("header={:?}",std::str::from_utf8(&b[..58]).unwrap());
    println!("byte10..18={:?}",std::str::from_utf8(&b[10..18]).unwrap());
    let f=parse_fcs(&b).unwrap();
    println!("{:?}", f.keywords);
}
