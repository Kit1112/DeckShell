//! RSA PKCS#1 v1.5 (тип 2) для пароля: Steam выдаёт модуль/экспоненту в hex (GetPasswordRSAPublicKey),
//! сток шифрует пароль и шлёт base64 (sp.js 53081 P8). Случайный паддинг — из /dev/urandom.
use num_bigint::BigUint;

fn random_nonzero(n: usize) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let mut f = std::fs::File::open("/dev/urandom")?;
    let mut out = Vec::with_capacity(n);
    let mut buf = [0u8; 64];
    while out.len() < n {
        f.read_exact(&mut buf)?;
        out.extend(buf.iter().copied().filter(|b| *b != 0).take(n - out.len()));
    }
    Ok(out)
}

/// Шифрует `msg` публичным ключом (hex-модуль, hex-экспонента). `None`: ключ неразборчив или сообщение длинное.
pub fn encrypt_pkcs1(modulus_hex: &str, exp_hex: &str, msg: &[u8]) -> Option<Vec<u8>> {
    let n = BigUint::parse_bytes(modulus_hex.trim().as_bytes(), 16)?;
    let e = BigUint::parse_bytes(exp_hex.trim().as_bytes(), 16)?;
    let k = n.to_bytes_be().len();
    if k < 64 || msg.len() > k - 11 { return None; }
    let pad = random_nonzero(k - 3 - msg.len()).ok()?;
    let mut em = Vec::with_capacity(k);
    em.extend_from_slice(&[0, 2]);
    em.extend_from_slice(&pad);
    em.push(0);
    em.extend_from_slice(msg);
    let c = BigUint::from_bytes_be(&em).modpow(&e, &n).to_bytes_be();
    let mut out = vec![0u8; k - c.len()];
    out.extend_from_slice(&c);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn decrypts_back_with_private_key_and_has_pkcs1_layout() {
        // Тестовая пара из простых Мерсенна M127 и M521 (648 бит); 65537 взаимно просто с φ (ord₆₅₅₃₇(2)=32 ∤ 126, 520).
        let one = BigUint::from(1u32);
        let p = (&one << 127usize) - &one;
        let q = (&one << 521usize) - &one;
        let n = &p * &q;
        let e = BigUint::from(65537u32);
        let phi = (&p - &one) * (&q - &one);
        let d = e.modinv(&phi).unwrap();
        let hex = n.to_str_radix(16);
        let a = encrypt_pkcs1(&hex, "010001", b"password").unwrap();
        let b = encrypt_pkcs1(&hex, "010001", b"password").unwrap();
        let k = n.to_bytes_be().len();
        assert_eq!(a.len(), k);
        assert_ne!(a, b, "паддинг случайный");
        let m = BigUint::from_bytes_be(&a).modpow(&d, &n).to_bytes_be();
        // Ведущий 0x00 теряется в to_bytes_be: остаётся 0x02 | паддинг без нулей | 0x00 | сообщение.
        assert_eq!(m.len(), k - 1);
        assert_eq!(m[0], 2);
        let sep = m.iter().position(|b| *b == 0).unwrap();
        assert_eq!(sep, k - 1 - 9);
        assert_eq!(&m[sep + 1..], b"password");
        assert!(encrypt_pkcs1(&hex, "010001", &vec![1u8; k - 10]).is_none(), "слишком длинное сообщение");
        assert!(encrypt_pkcs1("zz", "010001", b"x").is_none());
    }
}
