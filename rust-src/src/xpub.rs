//use bs58;
use bitcoin::Address;
use bitcoin::Network;
use bitcoin::ScriptBuf;
use bitcoin::WPubkeyHash;
use bitcoin::bip32::DerivationPath;
use bitcoin::bip32::Xpub;
use bitcoin::hashes::Hash;
use bitcoin::key::Secp256k1;
use sha2::{Digest, Sha256};
use std::str::FromStr;

// Mainnet (BIP44/BIP49/BIP84)
#[allow(dead_code)]
enum BS58Prefix {
    Xpub,
    Ypub,
    Zpub,
    Tpub,
    Vpub,
    Upub,
}
const XPUB_PREFIX: [u8; 4] = [0x04, 0x88, 0xB2, 0x1E]; // xpub (Legacy P2PKH)
const YPUB_PREFIX: [u8; 4] = [0x04, 0x9D, 0x7C, 0xB2]; // ypub (Nested SegWit P2SH-P2WPKH)
const ZPUB_PREFIX: [u8; 4] = [0x04, 0xB2, 0x47, 0x46]; // zpub (Native SegWit P2WPKH)
const TPUB_PREFIX: [u8; 4] = [0x04, 0x35, 0x87, 0xCF]; // tpub (Testnet Legacy P2PKH)
const VPUB_PREFIX: [u8; 4] = [0x04, 0x5F, 0x1C, 0xF6]; // vpub (Testnet Nested SegWit)
const UPUB_PREFIX: [u8; 4] = [0x04, 0x4A, 0x52, 0x62]; // upub (RegTest Nested SegWit)
// Constants from Bitcoin Core's checksum algorithm
const INPUT_CHARSET: &[u8] = b"0123456789()[],'/*abcdefgh@:$%{}IJKLMNOPQRSTUVWXYZ&+-.;<=>?!^_|~ijklmnopqrstuvwxyzABCDEFGH`#\"\\ ";
const CHECKSUM_CHARSET: &[u8] = b"qpzry9x8gf2tvdw0s3jn54khce6mua7l";

// Polynomial modulo function used in checksum calculation (same as in Bitcoin Core)
fn poly_mod(mut c: u64, val: u64) -> u64 {
    let c0 = c >> 35;
    c = ((c & 0x7ffffffff) << 5) ^ val;
    if c0 & 1 > 0 {
        c ^= 0xf5dee51989
    };
    if c0 & 2 > 0 {
        c ^= 0xa9fdca3312
    };
    if c0 & 4 > 0 {
        c ^= 0x1bab10e32d
    };
    if c0 & 8 > 0 {
        c ^= 0x3706b1677a
    };
    if c0 & 16 > 0 {
        c ^= 0x644d626ffd
    };

    c
}

// Calculate checksum for a descriptor string
fn calc_checksum(desc: &str) -> Result<String, String> {
    // Separate descriptor from any existing checksum
    let desc = match desc.split_once('#') {
        Some((d, _)) => d,
        None => desc,
    };

    let mut c: u64 = 1;
    let mut cls: u64 = 0;
    let mut clscount: u64 = 0;

    // Process each character in the descriptor
    for ch in desc.as_bytes() {
        let pos = match INPUT_CHARSET.iter().position(|b| b == ch) {
            Some(p) => p as u64,
            None => return Err(format!("Invalid character in descriptor: {}", *ch as char)),
        };

        c = poly_mod(c, pos & 31);
        cls = cls * 3 + (pos >> 5);
        clscount += 1;

        if clscount == 3 {
            c = poly_mod(c, cls);
            cls = 0;
            clscount = 0;
        }
    }

    if clscount > 0 {
        c = poly_mod(c, cls);
    }

    // Final steps in checksum calculation
    for _ in 0..8 {
        c = poly_mod(c, 0);
    }
    c ^= 1;

    // Convert checksum to characters
    let mut checksum = String::with_capacity(8);
    for j in 0..8 {
        let idx = ((c >> (5 * (7 - j))) & 31) as usize;
        checksum.push(CHECKSUM_CHARSET[idx] as char);
    }

    Ok(checksum)
}

pub fn get_bitcoincore_descriptor(xpub: &str) -> String {
    let fingerprint = match calculate_fingerprint(xpub) {
        Ok(f) => f,
        Err(_) => return String::new(), // Invalid xpub, return empty descriptor
    };

    let xpub_converted = match convert_xpub(xpub) {
        Ok(c) => c,
        Err(_) => return String::new(), // Invalid xpub, return empty descriptor
    };
    let descriptor = format!("wpkh([{}/84h/0h/0h]{}/0/*)", fingerprint, xpub_converted);
    match calc_checksum(&descriptor) {
        Ok(checksum) => {
            let clean_descriptor = descriptor.split('#').next().unwrap_or(&descriptor);
            format!("{}#{}", clean_descriptor, checksum)
        }
        Err(err) => {
            eprintln!("Error: {}", err);
            String::new()
        }
    }
}
fn convert_xpub(xpub: &str) -> Result<String, String> {
    if xpub.len() >= 4 && (&xpub[0..4] == "xpub" || &xpub[0..4] == "ypub" || &xpub[0..4] == "zpub")
    {
        convert_to(xpub, BS58Prefix::Xpub)
    } else if xpub.len() >= 4
        && (&xpub[0..4] == "tpub" || &xpub[0..4] == "vpub" || &xpub[0..4] == "upub")
    {
        convert_to(xpub, BS58Prefix::Tpub)
    } else {
        Err("Invalid xpub prefix: expected xpub, ypub, zpub, tpub, vpub, or upub".to_string())
    }
}
pub fn calculate_fingerprint(tpub: &str) -> Result<String, String> {
    let xpub = Xpub::from_str(&convert_to(tpub, BS58Prefix::Xpub)?)
        .map_err(|e| format!("Invalid xpub: {}", e))?;
    let fp = xpub.fingerprint();
    let _pp = xpub.parent_fingerprint;
    Ok(format!("{}", fp))
}

fn base58check_decode(s: &str) -> Result<Vec<u8>, String> {
    let data = bs58::decode(s).into_vec().map_err(|e| e.to_string())?;
    if data.len() < 4 {
        return Err("Data troppo corta".to_string());
    }
    let (payload, checksum) = data.split_at(data.len() - 4);
    let hash = Sha256::digest(Sha256::digest(payload));
    if hash[0..4] != checksum[..] {
        return Err("Checksum invalido".to_string());
    }
    Ok(payload.to_vec())
}

fn base58check_encode(data: &[u8]) -> String {
    let checksum = &Sha256::digest(Sha256::digest(data))[0..4];
    let full = [data, checksum].concat();
    bs58::encode(full).into_string()
}

fn convert_to(zpub: &str, prefix: BS58Prefix) -> Result<String, String> {
    let mut data = base58check_decode(zpub)?;

    if data.len() < 4 {
        return Err("Non è una zpub valida.".to_string());
    }
    data.splice(
        0..4,
        match prefix {
            BS58Prefix::Xpub => XPUB_PREFIX,
            BS58Prefix::Ypub => YPUB_PREFIX,
            BS58Prefix::Zpub => ZPUB_PREFIX,
            BS58Prefix::Vpub => VPUB_PREFIX,
            BS58Prefix::Tpub => TPUB_PREFIX,
            BS58Prefix::Upub => UPUB_PREFIX,
        },
    );

    Ok(base58check_encode(&data))
}
pub fn new_address_from_xpub(
    zpub: &str,
    index: i64,
    network: Network,
) -> Result<(String, String), Box<dyn std::error::Error>> {
    let xpub = Xpub::from_str(&convert_to(zpub, BS58Prefix::Xpub)?)?;
    let path = format!("m/0/{}", index);
    let derivation_path = DerivationPath::from_str(path.as_str())?;
    let secp = Secp256k1::new();
    let derived_xpub = xpub.derive_pub(&secp, &derivation_path)?;
    let public_key = derived_xpub.public_key;
    let pubkey_bytes = public_key.serialize();
    let witness_program = WPubkeyHash::hash(&pubkey_bytes);
    let redeem_script = ScriptBuf::new_p2wpkh(&witness_program);
    //let script_pubkey = ScriptBuf::new_p2sh(&redeem_script.script_hash());
    let address = Address::from_script(&redeem_script, network)?;
    //let address = Address::from_script(&script_pubkey, network)?;
    Ok((address.to_string(), path.to_string()))
}
/*
fn main() -> Result<(), Box<dyn std::error::Error>>{
    match convert_to(zpub,BS58Prefix::Tpub) {
        Ok(tpub) => println!("XPUB: {}", tpub),
        Err(e) => eprintln!("Errore: {}", e),
    }
    let fingerprint = base58check_encode(&calculate_fingerprint(zpub));
    println!("ZPUB: {}, FINGERPRINT: {}",zpub,fingerprint);

    let xpub = Xpub::from_str(&convert_to(zpub,BS58Prefix::Xpub)?)?;
    let tpub = convert_to(zpub,BS58Prefix::Tpub)?;
    let fingerprint = base58check_encode(&calculate_fingerprint(&tpub));
    println!("TPUB: {}, FINGERPRINT: {}",tpub,fingerprint);
    let derivation_path = DerivationPath::from_str("m/0/0")?;
    let secp = Secp256k1::new();
    let derived_xpub = xpub.derive_pub(&secp, &derivation_path)?;

    let public_key = derived_xpub.public_key;
    let pubkey_bytes = public_key.serialize();
    let witness_program = WPubkeyHash::hash(&pubkey_bytes);
    let redeem_script = ScriptBuf::new_p2wpkh(&witness_program);
    let script_pubkey = ScriptBuf::new_p2sh(&redeem_script.script_hash());

    // Generate the Bitcoin SegWit (BIP49) address
    let network = Network::Bitcoin;
    let address = Address::from_script(&redeem_script, network)?;
    let address = Address::from_script(&script_pubkey, network)?;

    Ok(())
}*/
