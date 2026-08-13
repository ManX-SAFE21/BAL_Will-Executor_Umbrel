/// Extended public key (xpub) utilities for Bitcoin After Life.
///
/// Supports all common xpub prefixes:
///   Mainnet:  xpub (Legacy P2PKH), ypub (Nested SegWit), zpub (Native SegWit)
///   Testnet:  tpub (Legacy), upub (Nested SegWit), vpub (Native SegWit)
///
/// Primary public function: `new_address_from_xpub(xpub, index, network)`
/// which derives the address at path m/0/{index} using BIP84 (native SegWit).
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

// --- Base58 version byte prefixes for different xpub types ---
// These 4-byte prefixes distinguish key formats for different address types.

/// xpub — Mainnet Legacy P2PKH (BIP44)
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

// --- Bitcoin Core descriptor checksum algorithm ---
// Matches Bitcoin Core's implementation for compatibility with importdescriptors.

const INPUT_CHARSET: &[u8] =
    b"0123456789()[],'/*abcdefgh@:$%{}IJKLMNOPQRSTUVWXYZ&+-.;<=>?!^_|~ijklmnopqrstuvwxyzABCDEFGH`#\"\\ ";
const CHECKSUM_CHARSET: &[u8] = b"qpzry9x8gf2tvdw0s3jn54khce6mua7l";

/// Polynomial modulo used in descriptor checksum calculation.
/// Mirrors Bitcoin Core's DescriptorChecksum implementation.
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

/// Calculate the 8-character checksum for a Bitcoin output descriptor.
/// Returns Err if the descriptor contains invalid characters.
fn calc_checksum(desc: &str) -> Result<String, String> {
    // Strip any existing checksum before recalculating
    let desc = match desc.split_once('#') {
        Some((d, _)) => d,
        None => desc,
    };

    let mut c: u64 = 1;
    let mut cls: u64 = 0;
    let mut clscount: u64 = 0;

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

    for _ in 0..8 {
        c = poly_mod(c, 0);
    }
    c ^= 1;

    let mut checksum = String::with_capacity(8);
    for j in 0..8 {
        let idx = ((c >> (5 * (7 - j))) & 31) as usize;
        checksum.push(CHECKSUM_CHARSET[idx] as char);
    }

    Ok(checksum)
}

/// Build a Bitcoin Core-compatible wpkh() descriptor for an xpub.
///
/// Format: `wpkh([fingerprint/84h/0h/0h]xpub/0/*)#checksum`
/// This descriptor can be imported into Bitcoin Core with `importdescriptors`.
pub fn get_bitcoincore_descriptor(xpub: &String) -> String {
    let fingerprint = calculate_fingerprint(xpub);
    let descriptor = format!(
        "wpkh([{}/84h/0h/0h]{}/0/*)",
        fingerprint,
        convert_xpub(xpub)
    );
    let descriptor = match calc_checksum(&descriptor) {
        Ok(checksum) => {
            let clean_descriptor = descriptor.split('#').next().unwrap_or(&descriptor);
            format!("{}#{}", clean_descriptor, checksum)
        }
        Err(err) => {
            eprintln!("Error: {}", err);
            "".to_string()
        }
    };
    descriptor
}

/// Convert any xpub variant (ypub, zpub, tpub, vpub, upub) to the canonical
/// xpub or tpub form that the bitcoin library understands.
fn convert_xpub(xpub: &String) -> String {
    if xpub[0..4] == *"xpub" || xpub[0..4] == *"ypub" || xpub[0..4] == *"zpub" {
        return convert_to(xpub, BS58Prefix::Xpub).unwrap();
    } else {
        return convert_to(xpub, BS58Prefix::Tpub).unwrap();
    }
}

/// Compute the key fingerprint (first 4 bytes of HASH160 of the public key).
/// Used in descriptor output as `[fingerprint/path]`.
pub fn calculate_fingerprint(tpub: &str) -> String {
    let xpub = Xpub::from_str(&convert_to(tpub, BS58Prefix::Xpub).unwrap()).unwrap();
    let fp = xpub.fingerprint();
    format!("{}", fp)
}

/// Decode a Base58Check-encoded string and verify its checksum.
/// Returns the payload (without the 4-byte checksum) on success.
fn base58check_decode(s: &str) -> Result<Vec<u8>, String> {
    let data = bs58::decode(s).into_vec().map_err(|e| e.to_string())?;
    if data.len() < 4 {
        return Err("Data too short".to_string());
    }
    let (payload, checksum) = data.split_at(data.len() - 4);
    let hash = Sha256::digest(&Sha256::digest(payload));
    if hash[0..4] != checksum[..] {
        return Err("Invalid checksum".to_string());
    }
    Ok(payload.to_vec())
}

/// Re-encode payload bytes as Base58Check (payload + 4-byte checksum).
fn base58check_encode(data: &[u8]) -> String {
    let checksum = &Sha256::digest(&Sha256::digest(data))[0..4];
    let full = [data, checksum].concat();
    bs58::encode(full).into_string()
}

/// Convert any xpub variant to a different prefix type.
/// This works by decoding the Base58Check, swapping the first 4 bytes,
/// and re-encoding. The key material itself is unchanged.
fn convert_to(zpub: &str, prefix: BS58Prefix) -> Result<String, String> {
    let mut data = base58check_decode(zpub)?;

    if data.len() < 4 {
        return Err("Not a valid xpub key.".to_string());
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

/// Derive a Native SegWit (bech32) address from an xpub at path m/0/{index}.
///
/// Accepts any xpub variant (xpub, ypub, zpub, tpub, vpub, upub) and
/// normalizes it internally to xpub before derivation.
///
/// Returns `(address, derivation_path)` on success.
pub fn new_address_from_xpub(
    zpub: &str,
    index: i64,
    network: Network,
) -> Result<(String, String), Box<dyn std::error::Error>> {
    let xpub = Xpub::from_str(&convert_to(zpub, BS58Prefix::Xpub)?)?;
    let path = format!("m/0/{}", index);
    let derivation_path = DerivationPath::from_str(&path.as_str())?;
    let secp = Secp256k1::new();
    let derived_xpub = xpub.derive_pub(&secp, &derivation_path)?;
    let public_key = derived_xpub.public_key;
    let pubkey_bytes = public_key.serialize();
    let witness_program = WPubkeyHash::hash(&pubkey_bytes);
    let redeem_script = ScriptBuf::new_p2wpkh(&witness_program);
    let address = Address::from_script(&redeem_script, network)?;
    Ok((address.to_string(), path.to_string()))
}
