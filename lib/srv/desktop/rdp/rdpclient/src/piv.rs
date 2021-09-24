use crate::errors::invalid_data_error;
use iso7816::aid::Aid;
use iso7816::command::instruction::Instruction;
use iso7816::command::Command;
use iso7816::response::Status;
use iso7816_tlv::ber::{Tag, Tlv, Value};
use openssl::pkey::Private;
use openssl::rsa::{Padding, Rsa};
use rdp::model::error::*;
use std::convert::TryFrom;
use std::io::{Cursor, Read};
use uuid::Uuid;

// AID of PIV application, per
// https://nvlpubs.nist.gov/nistpubs/SpecialPublications/NIST.SP.800-73-4.pdf
const PIV_AID: Aid = Aid::new_truncatable(
    &[
        0xA0, 0x00, 0x00, 0x03, 0x08, 0x00, 0x00, 0x10, 0x00, 0x01, 0x00,
    ],
    5, // usually truncates to first 5 bytes
);

#[derive(Debug)]
pub struct Card<const S: usize> {
    chuid: Vec<u8>,
    piv_auth_cert: Vec<u8>,
    piv_auth_key: Rsa<Private>,
    pending_command: Option<Command<S>>,
    pending_response: Option<Cursor<Vec<u8>>>,
}

impl<const S: usize> Card<S> {
    pub fn new(uuid: Uuid, cert_der: &[u8], key_der: &[u8]) -> RdpResult<Self> {
        Ok(Self {
            chuid: Self::build_chuid(uuid),
            piv_auth_cert: Self::build_piv_auth_cert(cert_der),
            piv_auth_key: Rsa::private_key_from_der(key_der).or_else(|e| {
                Err(invalid_data_error(&format!(
                    "failed to parse private key from DER: {:?}",
                    e
                )))
            })?,
            pending_command: None,
            pending_response: None,
        })
    }

    pub fn handle(&mut self, cmd: Command<S>) -> RdpResult<Response> {
        debug!("got command: {:?}", cmd);
        debug!("command data: {}", hex_data(&cmd));

        // Handle chained commands.
        let cmd = match self.pending_command.as_mut() {
            None => cmd,
            Some(pending) => {
                pending
                    .extend_from_command(&cmd)
                    .or_else(|_| Err(invalid_data_error("could not build chained command")))?;
                pending.clone()
            }
        };
        if cmd.class().chain().not_the_last() {
            self.pending_command = Some(cmd);
            return Ok(Response::new(Status::Success));
        } else {
            self.pending_command = None;
        }

        let resp = match cmd.instruction() {
            Instruction::Select => self.handle_select(cmd),
            Instruction::Verify => self.handle_verify(cmd),
            Instruction::GetData => self.handle_get_data(cmd),
            Instruction::GetResponse => self.handle_get_response(cmd),
            Instruction::GeneralAuthenticate => self.handle_general_authenticate(cmd),
            _ => {
                warn!("unimplemented instruction {:?}", cmd.instruction());
                Ok(Response::new(Status::InstructionNotSupportedOrInvalid))
            }
        }?;
        debug!("send response: {:?}", resp);
        debug!("response data: {}", to_hex(&resp.encode()));
        Ok(resp)
    }

    fn handle_select(&mut self, cmd: Command<S>) -> RdpResult<Response> {
        // For our use case, we only allow selecting the PIV application on the smartcard.
        //
        // P1=04 and P2=00 means selection of DF (usually) application by name. Everything else not
        // supported.
        if cmd.p1 != 0x04 && cmd.p2 != 0x00 {
            return Ok(Response::new(Status::NotFound));
        }
        if !PIV_AID.matches(cmd.data()) {
            return Ok(Response::new(Status::NotFound));
        }

        // See https://nvlpubs.nist.gov/nistpubs/SpecialPublications/NIST.SP.800-73-4.pdf section
        // 3.1.1
        let resp = tlv(
            0x61,
            Value::Constructed(vec![
                tlv(
                    0x4F,
                    Value::Primitive(vec![0x00, 0x00, 0x10, 0x00, 0x01, 0x00]),
                )?,
                tlv(
                    0x79,
                    Value::Constructed(vec![tlv(
                        0x4F,
                        Value::Primitive(PIV_AID.truncated().to_vec()),
                    )?]),
                )?,
            ]),
        )?;
        Ok(Response::with_data(Status::Success, resp.to_vec().into()))
    }

    fn handle_verify(&mut self, _cmd: Command<S>) -> RdpResult<Response> {
        // No PIN verification needed.
        Ok(Response::new(Status::Success))
    }

    fn handle_get_data(&mut self, cmd: Command<S>) -> RdpResult<Response> {
        // See https://nvlpubs.nist.gov/nistpubs/SpecialPublications/NIST.SP.800-73-4.pdf section
        // 3.1.2.
        if cmd.p1 != 0x3F && cmd.p2 != 0xFF {
            return Ok(Response::new(Status::NotFound));
        }
        let request_tlv = Tlv::from_bytes(cmd.data())
            .or_else(|e| Err(invalid_data_error(&format!("TLV invalid: {:?}", e))))?;
        if *request_tlv.tag() != tlv_tag(0x5C)? {
            return Ok(Response::new(Status::NotFound));
        }
        match request_tlv.value() {
            Value::Primitive(tag) => match to_hex(&tag).as_str() {
                // Card Holder Unique Identifier.
                "5FC102" => Ok(Response::with_data(Status::Success, self.chuid.clone())),
                // X.509 Certificate for PIV Authentication
                "5FC105" => {
                    self.pending_response = Some(Cursor::new(self.piv_auth_cert.clone()));
                    self.handle_get_response(cmd)
                }
                _ => {
                    // Some other unimplemented data object.
                    Ok(Response::new(Status::NotFound))
                }
            },
            Value::Constructed(_) => Ok(Response::new(Status::NotFound)),
        }
    }

    fn handle_get_response(&mut self, _cmd: Command<S>) -> RdpResult<Response> {
        match &mut self.pending_response {
            None => Ok(Response::new(Status::NotFound)),
            Some(cursor) => {
                let mut chunk = [0; 256];
                let n = cursor.read(&mut chunk)?;
                let mut chunk = chunk.to_vec();
                chunk.truncate(n);
                let remaining = cursor.get_ref().len() as u64 - cursor.position();
                let status = if remaining <= 0 {
                    Status::Success
                } else if remaining < 256 {
                    Status::MoreAvailable(remaining as u8)
                } else {
                    Status::MoreAvailable(0)
                };
                Ok(Response::with_data(status, chunk))
            }
        }
    }

    fn handle_general_authenticate(&mut self, cmd: Command<S>) -> RdpResult<Response> {
        // See section 3.2.4 and example in Appending A.3 from
        // https://nvlpubs.nist.gov/nistpubs/SpecialPublications/NIST.SP.800-73-4.pdf

        // P1='07' means 2048-bit RSA.
        //
        // TODO(awly): compare algorithm against the private key using consts from
        // https://nvlpubs.nist.gov/nistpubs/SpecialPublications/NIST.SP.800-78-4.pdf
        if cmd.p1 != 0x07 {
            return Err(invalid_data_error(&format!(
                "unsupported algorithm identifier P1:{:#X} in general authenticate command",
                cmd.p1
            )));
        }
        // P2='9A' means PIV Authentication Key (matches our cert '5FC105' in handle_get_data).
        if cmd.p2 != 0x9A {
            return Err(invalid_data_error(&format!(
                "unsupported key reference P2:{:#X} in general authenticate command",
                cmd.p2
            )));
        }

        let request_tlv = Tlv::from_bytes(cmd.data())
            .or_else(|e| Err(invalid_data_error(&format!("TLV invalid: {:?}", e))))?;
        if *request_tlv.tag() != tlv_tag(0x7C)? {
            return Err(invalid_data_error(&format!(
                "general authenticate command TLV invalid: {:?}",
                request_tlv
            )));
        }

        // Extract the challenge field.
        let request_tlvs = match request_tlv.value() {
            Value::Primitive(_) => {
                return Err(invalid_data_error(&format!(
                    "general authenticate command TLV invalid: {:?}",
                    request_tlv
                )));
            }
            Value::Constructed(tlvs) => tlvs,
        };
        let mut challenge = None;
        for data in request_tlvs {
            if *data.tag() != tlv_tag(0x81)? {
                continue;
            }
            challenge = match data.value() {
                Value::Primitive(chal) => Some(chal),
                Value::Constructed(_) => {
                    return Err(invalid_data_error(&format!(
                        "general authenticate command TLV invalid: {:?}",
                        request_tlv
                    )));
                }
            };
        }
        let challenge = challenge.ok_or_else(|| {
            invalid_data_error(&format!(
                "general authenticate command TLV invalid: {:?}, missing challenge data",
                request_tlv
            ))
        })?;

        // Sign the challenge.
        let mut signed_challenge = Vec::new();
        signed_challenge.resize(self.piv_auth_key.size() as usize, 0);
        // This signature uses very low-level RSA primitives.
        //
        // For signatures, typically, you'd use openssl::sign::Signer with plaintext input data to
        // sign. Internally, the signer hashes the input, adds padding according to some scheme
        // (like PKCS1v15 or PSS) and then "decrypts" this data with the key. The decrypted blob is
        // the signature.
        //
        // In our case, the RDP server does all of the above hashing and signing and only gives us
        // a finished blob to decrypt. This is why we call private_decrypt below, and not the usual
        // signer.
        self.piv_auth_key
            .private_decrypt(&challenge, &mut signed_challenge, Padding::NONE)
            .or_else(|e| {
                Err(invalid_data_error(&format!(
                    "failed to sign challenge: {:?}",
                    e
                )))
            })?;

        // Return signed challenge.
        let resp = tlv(
            0x7C,
            Value::Constructed(vec![tlv(0x82, Value::Primitive(signed_challenge))?]),
        )?
        .to_vec();
        self.pending_response = Some(Cursor::new(resp));
        self.handle_get_response(cmd)
    }

    fn build_chuid(uuid: Uuid) -> Vec<u8> {
        // This is gross: the response is a BER-TLV value, but it has nested SIMPLE-TLV
        // values. None of the TLV encoding libraries out there support this, they fail
        // when checking the tag of nested TLVs.
        //
        // So, construct the TLV by hand from raw bytes. Hopefully the comments will be
        // enough to explain the structure.
        //
        // https://nvlpubs.nist.gov/nistpubs/SpecialPublications/NIST.SP.800-73-4.pdf
        // table 9 has the explanation of fields.
        //
        // Start with a top-level BER-TLV tag and length:
        let mut resp = vec![0x53, 0x3B];
        // TLV tag and length for FASC-N.
        resp.extend_from_slice(&[0x30, 0x19]);
        // FASC-N value containing S9999F9999F999999F0F1F0000000000300001E, with a
        // weird encoding from section 6 of:
        // https://www.idmanagement.gov/docs/pacs-tig-scepacs.pdf
        resp.extend_from_slice(&[
            0xd4, 0xe7, 0x39, 0xda, 0x73, 0x9c, 0xed, 0x39, 0xce, 0x73, 0x9d, 0x83, 0x68, 0x58,
            0x21, 0x08, 0x42, 0x10, 0x84, 0x21, 0xc8, 0x42, 0x10, 0xc3, 0xeb,
        ]);
        // TLV for user UUID.
        resp.extend_from_slice(&[0x34, 0x10]);
        resp.extend_from_slice(uuid.as_bytes());
        // TLV for expiration date (YYYYMMDD).
        resp.extend_from_slice(&[0x35, 0x08]);
        // TODO(awly): generate this from current time.
        resp.extend_from_slice("20300101".as_bytes());
        // TLV for signature (empty).
        resp.extend_from_slice(&[0x3E, 0x00]);
        // TLV for error detection code.
        resp.extend_from_slice(&[0xFE, 0x00]);
        resp
    }

    fn build_piv_auth_cert(cert_der: &[u8]) -> Vec<u8> {
        // Same as above, tags in this BER-TLV value are not compatible with the spec
        // and existing libraries. Marshal by hand.
        //
        // Certificate TLV tag and length.
        let mut resp = vec![0x70];
        resp.extend_from_slice(&len_to_vec(cert_der.len()));
        resp.extend_from_slice(cert_der);
        // CertInfo TLV (0x00 indicates uncompressed cert).
        resp.extend_from_slice(&[0x71, 0x01, 0x00]);
        // TLV for error detection code.
        resp.extend_from_slice(&[0xFE, 0x00]);

        // Wrap with top-level TLV tag and length.
        let mut resp_outer = vec![0x53];
        resp_outer.extend_from_slice(&len_to_vec(resp.len()));
        resp_outer.extend_from_slice(&resp);
        resp_outer
    }
}

#[derive(Debug)]
pub struct Response {
    data: Option<Vec<u8>>,
    status: Status,
}

impl Response {
    fn new(status: Status) -> Self {
        Self { data: None, status }
    }
    fn with_data(status: Status, data: Vec<u8>) -> Self {
        Self {
            data: Some(data),
            status,
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        if let Some(data) = &self.data {
            buf.extend_from_slice(&data);
        }
        let status: [u8; 2] = self.status.into();
        buf.extend_from_slice(&status);
        buf
    }
}

fn tlv(tag: u8, value: Value) -> RdpResult<Tlv> {
    Tlv::new(tlv_tag(tag)?, value).or_else(|e| {
        Err(invalid_data_error(&format!(
            "TLV with tag {:#X} invalid: {:?}",
            tag, e
        )))
    })
}

fn tlv_tag(val: u8) -> RdpResult<Tag> {
    Tag::try_from(val).or_else(|e| {
        Err(invalid_data_error(&format!(
            "TLV tag {:#X} invalid: {:?}",
            val, e
        )))
    })
}

fn hex_data<const S: usize>(cmd: &Command<S>) -> String {
    to_hex(&cmd.data().to_vec())
}

fn to_hex(bytes: &[u8]) -> String {
    let mut s = String::new();
    for b in bytes {
        s.push_str(&format!("{:02X}", b));
    }
    s
}

#[allow(clippy::cast_possible_truncation)]
fn len_to_vec(len: usize) -> Vec<u8> {
    if len < 0x7f {
        vec![len as u8]
    } else {
        let mut ret: Vec<u8> = len
            .to_be_bytes()
            .iter()
            .skip_while(|&x| *x == 0)
            .cloned()
            .collect();
        ret.insert(0, 0x80 | ret.len() as u8);
        ret
    }
}
