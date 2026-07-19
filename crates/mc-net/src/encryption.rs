use aes::{Aes128, cipher::KeyIvInit};
use cfb8::{Decryptor, Encryptor};

/// Stateful AES-128-CFB8 cipher pair used by the Minecraft transport.
///
/// Minecraft uses the shared secret as both the AES key and initialization
/// vector. Encryption and decryption are independent byte streams, so each
/// direction keeps its own feedback state.
pub struct MinecraftCipher {
    encryptor: Encryptor<Aes128>,
    decryptor: Decryptor<Aes128>,
}

impl MinecraftCipher {
    /// Creates fresh encryption and decryption streams from a shared secret.
    #[must_use]
    pub fn new(shared_secret: &[u8; 16]) -> Self {
        Self {
            encryptor: Encryptor::new(shared_secret.into(), shared_secret.into()),
            decryptor: Decryptor::new(shared_secret.into(), shared_secret.into()),
        }
    }

    /// Encrypts the next bytes in the outbound stream.
    pub fn encrypt_in_place(&mut self, bytes: &mut [u8]) {
        self.encryptor.encrypt(bytes);
    }

    /// Decrypts the next bytes in the inbound stream.
    pub fn decrypt_in_place(&mut self, bytes: &mut [u8]) {
        self.decryptor.decrypt(bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::MinecraftCipher;

    const SHARED_SECRET: [u8; 16] = [
        0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e,
        0x0f,
    ];

    #[test]
    fn encrypts_known_aes_128_cfb8_vector() {
        let mut plaintext = [
            0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd,
            0xee, 0xff, 0x10, 0x20, 0x30, 0x40, 0x50, 0x60, 0x70, 0x80,
        ];
        let expected = [
            0x0a, 0x32, 0x29, 0xc3, 0x90, 0x9d, 0xfb, 0x17, 0xb1, 0xf4, 0xd6, 0x35, 0xa0, 0x01,
            0x76, 0xa3, 0xb4, 0x84, 0xd8, 0x26, 0x0e, 0x56, 0x6e, 0xc8,
        ];

        let mut cipher = MinecraftCipher::new(&SHARED_SECRET);
        cipher.encrypt_in_place(&mut plaintext);

        assert_eq!(plaintext, expected);
    }

    #[test]
    fn decrypts_ciphertext_back_to_plaintext() {
        let plaintext = b"a minecraft packet split across transport reads";
        let mut ciphertext = *plaintext;
        let mut encryptor = MinecraftCipher::new(&SHARED_SECRET);
        encryptor.encrypt_in_place(&mut ciphertext);

        let mut decryptor = MinecraftCipher::new(&SHARED_SECRET);
        decryptor.decrypt_in_place(&mut ciphertext);

        assert_eq!(&ciphertext, plaintext);
    }

    #[test]
    fn chunked_encryption_matches_whole_buffer_encryption() {
        let plaintext = b"packet one followed by packet two in one encrypted TCP stream";
        let mut whole = *plaintext;
        let mut chunked = *plaintext;

        let mut whole_cipher = MinecraftCipher::new(&SHARED_SECRET);
        whole_cipher.encrypt_in_place(&mut whole);

        let mut chunked_cipher = MinecraftCipher::new(&SHARED_SECRET);
        let (first, remaining) = chunked.split_at_mut(7);
        let (second, third) = remaining.split_at_mut(19);
        chunked_cipher.encrypt_in_place(first);
        chunked_cipher.encrypt_in_place(second);
        chunked_cipher.encrypt_in_place(third);

        assert_eq!(chunked, whole);
    }

    #[test]
    fn encrypt_and_decrypt_directions_keep_independent_state() {
        let client_plaintext = b"serverbound bytes";
        let server_plaintext = b"clientbound bytes from a separate stream";
        let mut client_ciphertext = *client_plaintext;
        let mut server_ciphertext = *server_plaintext;

        let mut peer_encryptor = MinecraftCipher::new(&SHARED_SECRET);
        peer_encryptor.encrypt_in_place(&mut client_ciphertext);
        let mut transport = MinecraftCipher::new(&SHARED_SECRET);
        transport.decrypt_in_place(&mut client_ciphertext);
        transport.encrypt_in_place(&mut server_ciphertext);

        let mut expected_server_ciphertext = *server_plaintext;
        let mut fresh_encryptor = MinecraftCipher::new(&SHARED_SECRET);
        fresh_encryptor.encrypt_in_place(&mut expected_server_ciphertext);

        assert_eq!(&client_ciphertext, client_plaintext);
        assert_eq!(server_ciphertext, expected_server_ciphertext);
    }
}
