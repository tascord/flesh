use {
    heck::AsPascalCase,
    itertools::Itertools,
    proc_macro2::TokenStream,
    quote::{format_ident, quote},
    std::{fs::OpenOptions, io::Write, ops::Not, process::Command},
};

const CSV_TARGET: &str = "./statuses.csv";
const TARGET_FILE: &str = "./src/transport/status.rs";

fn main() {
    let mut rdr = csv::Reader::from_reader(OpenOptions::new().read(true).open(CSV_TARGET).unwrap());
    let (enum_fields, (into_arms, cat_arms)): (Vec<TokenStream>, (Vec<TokenStream>, Vec<TokenStream>)) = rdr
        .deserialize::<(String, String, String, String, String)>()
        .filter_map(|v| v.ok().and_then(|v| v.3.is_empty().not().then_some(parse_csv_line(v))))
        .unzip();

    let len = enum_fields.len();
    let selfs = into_arms.clone().iter().map(|v| v.clone().into_iter().take(4).collect::<TokenStream>()).collect_vec();

    let file = quote! {

        #[derive(Clone, Copy, Debug)]
        pub enum Status {
            #(#enum_fields)*
            Custom(u8),
        }

        impl Status {
            pub const STANDARD: [Self;#len]= [#(#selfs,)*];

            pub fn as_u8(&self) -> u8 {
                match self {
                    #(#into_arms)*
                    Self::Custom(int) => *int
                }
            }

            pub fn as_type(&self) -> StatusType {
                match self {
                    #(#cat_arms)*
                    Self::Custom(_) => StatusType::Unknown
                }
            }

            pub fn is_ok(&self) -> bool {
                match self.as_type() {
                    StatusType::Routing | StatusType::Hints | StatusType::Oks => true,
                    _ => false
                }
            }
        }

        //

        #[derive(Clone, Copy, Debug)]
        pub enum StatusType {
            /// 001 -> 014
            Routing,
            /// 015 -> 020
            RoutingError,
            /// 021 -> 030
            Hints,
            /// 031 -> 040
            Oks,
            /// 041 -> 050
            ClientErrors,
            /// 051 -> 060
            ServerErrors,
            /// Currently unbound or in custom range 061->254(~)
            Unknown
        }

    };

    OpenOptions::new()
        .create_new(false)
        .write(true)
        .truncate(true)
        .open(TARGET_FILE)
        .unwrap()
        .write_all(file.to_string().as_bytes())
        .unwrap();

    Command::new("rustfmt").arg(TARGET_FILE).spawn().unwrap().wait().unwrap();
}

fn parse_csv_line(rec: (String, String, String, String, String)) -> (TokenStream, (TokenStream, TokenStream)) {
    let (int, cat, equiv, ident, mut note) = rec;
    if ident.is_empty() {
        return (TokenStream::new(), (TokenStream::new(), TokenStream::new()));
    }

    let int = int.parse::<u8>().unwrap();
    let ident = format_ident!("{}", AsPascalCase(ident).to_string());

    if !equiv.is_empty() {
        note = format!("{note} (HTTP Equivalent {equiv})");
    }

    let docstr = format!("[{int:0>3}] -- {note}");
    let enum_field = quote! {
        #[doc = #docstr]
        #ident,
    };

    let to_u8_arm = quote! {
        Self::#ident => #int,
    };

    let cat = format_ident!("{}", AsPascalCase(cat).to_string());
    let cat_arm = quote! {
        Self::#ident => StatusType::#cat,
    };

    (enum_field, (to_u8_arm, cat_arm))
}
