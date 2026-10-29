//! Cases of AWS's Signature Version 4 test suite (`aws-sig-v4-test-suite`, 2015-08-30): the
//! request, its canonical request, the string to sign and the `Authorization` header. The
//! signer adds `Host` (from the URL) and `X-Amz-Date`, and `X-Amz-Security-Token` when the
//! credentials carry a session token (`post-sts-header-before`).

pub const KEY_ID: &str = "AKIDEXAMPLE";
pub const SECRET: &str = "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY";
pub const REGION: &str = "us-east-1";
pub const SERVICE: &str = "service";
pub const HOST: &str = "example.amazonaws.com";
/// 2015-08-30 12:36:00 UTC in seconds since the epoch.
pub const TIME: i64 = 1_440_938_160;
pub const SESSION_TOKEN: &str = "AQoDYXdzEPT//////////wEXAMPLEtc764bNrC9SAPBSM22wDOk4x4HIZ8j4FZTwdQWLWsKWHGBuFqwAeMicRXmxfpSPfIeoIYRqTflfKD8YUuwthAx7mSEI/qkPpKPi/kMcGdQrmGdeehM4IC1NtBmUpp2wUE8phUZampKsburEDy0KPkyQDYwT7WZ0wq5VSXDvp75YU9HFvlRd8Tx6q6fE8YQcHNVXAkiY9q6d+xo0rKwT38xVqr7ZD0u0iPPkUL64lIZbqBAz+scqKmlzm8FDrypNC9Yjc8fPOLn9FX9KSYvKTr4rvx3iSIlTJabIQwj2ICCR/oLxBA==";

pub struct Sigv4Vector {
    pub name: &'static str,
    pub method: &'static str,
    /// Path and query as sent.
    pub target: &'static str,
    /// Headers besides the ones the signer adds.
    pub headers: &'static [(&'static str, &'static str)],
    pub body: &'static str,
    pub session_token: Option<&'static str>,
    pub canonical_request: &'static str,
    pub string_to_sign: &'static str,
    pub authorization: &'static str,
}

pub const VECTORS: [Sigv4Vector; 4] = [
    Sigv4Vector {
        name: "get-vanilla",
        method: "GET",
        target: "/",
        headers: &[],
        body: "",
        session_token: None,
        canonical_request: "GET\n/\n\nhost:example.amazonaws.com\nx-amz-date:20150830T123600Z\n\nhost;x-amz-date\ne3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        string_to_sign: "AWS4-HMAC-SHA256\n20150830T123600Z\n20150830/us-east-1/service/aws4_request\nbb579772317eb040ac9ed261061d46c1f17a8133879d6129b6e1c25292927e63",
        authorization: "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/service/aws4_request, SignedHeaders=host;x-amz-date, Signature=5fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31",
    },
    Sigv4Vector {
        name: "get-vanilla-query-order-key-case",
        method: "GET",
        target: "/?Param2=value2&Param1=value1",
        headers: &[],
        body: "",
        session_token: None,
        canonical_request: "GET\n/\nParam1=value1&Param2=value2\nhost:example.amazonaws.com\nx-amz-date:20150830T123600Z\n\nhost;x-amz-date\ne3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        string_to_sign: "AWS4-HMAC-SHA256\n20150830T123600Z\n20150830/us-east-1/service/aws4_request\n816cd5b414d056048ba4f7c5386d6e0533120fb1fcfa93762cf0fc39e2cf19e0",
        authorization: "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/service/aws4_request, SignedHeaders=host;x-amz-date, Signature=b97d918cfa904a5beff61c982a1b6f458b799221646efd99d3219ec94cdf2500",
    },
    Sigv4Vector {
        name: "post-x-www-form-urlencoded",
        method: "POST",
        target: "/",
        headers: &[("Content-Type", "application/x-www-form-urlencoded")],
        body: "Param1=value1",
        session_token: None,
        canonical_request: "POST\n/\n\ncontent-type:application/x-www-form-urlencoded\nhost:example.amazonaws.com\nx-amz-date:20150830T123600Z\n\ncontent-type;host;x-amz-date\n9095672bbd1f56dfc5b65f3e153adc8731a4a654192329106275f4c7b24d0b6e",
        string_to_sign: "AWS4-HMAC-SHA256\n20150830T123600Z\n20150830/us-east-1/service/aws4_request\n42a5e5bb34198acb3e84da4f085bb7927f2bc277ca766e6d19c73c2154021281",
        authorization: "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/service/aws4_request, SignedHeaders=content-type;host;x-amz-date, Signature=ff11897932ad3f4e8b18135d722051e5ac45fc38421b1da7b9d196a0fe09473a",
    },
    Sigv4Vector {
        name: "post-sts-header-before",
        method: "POST",
        target: "/",
        headers: &[],
        body: "",
        session_token: Some(SESSION_TOKEN),
        canonical_request: concat!(
            "POST
/

host:example.amazonaws.com
x-amz-date:20150830T123600Z
x-amz-security-token:",
            "AQoDYXdzEPT//////////wEXAMPLEtc764bNrC9SAPBSM22wDOk4x4HIZ8j4FZTwdQWLWsKWHGBuFqwAeMicRXmxfpSPfIeoIYRqTflfKD8YUuwthAx7mSEI/qkPpKPi/kMcGdQrmGdeehM4IC1NtBmUpp2wUE8phUZampKsburEDy0KPkyQDYwT7WZ0wq5VSXDvp75YU9HFvlRd8Tx6q6fE8YQcHNVXAkiY9q6d+xo0rKwT38xVqr7ZD0u0iPPkUL64lIZbqBAz+scqKmlzm8FDrypNC9Yjc8fPOLn9FX9KSYvKTr4rvx3iSIlTJabIQwj2ICCR/oLxBA==",
            "

host;x-amz-date;x-amz-security-token
e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        ),
        string_to_sign: "AWS4-HMAC-SHA256\n20150830T123600Z\n20150830/us-east-1/service/aws4_request\nc237e1b440d4c63c32ca95b5b99481081cb7b13c7e40434868e71567c1a882f6",
        authorization: "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/service/aws4_request, SignedHeaders=host;x-amz-date;x-amz-security-token, Signature=85d96828115b5dc0cfc3bd16ad9e210dd772bbebba041836c64533a82be05ead",
    },
];
