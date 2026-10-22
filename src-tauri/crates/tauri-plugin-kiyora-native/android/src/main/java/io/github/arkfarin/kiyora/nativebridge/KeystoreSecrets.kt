package io.github.arkfarin.kiyora.nativebridge

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.AtomicFile
import java.io.File
import java.io.FileNotFoundException
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/** Keystore wraps 32-byte secrets; only authenticated ciphertext reaches disk. */
internal class KeystoreSecrets(
    context: Context,
    private val directory: File = File(context.noBackupFilesDir, "kiyora-secrets-v1"),
    private val aliasPrefix: String = "io.github.arkfarin.kiyora.secret.v1."
) {
    companion object {
        private val NAME = Regex("[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}:(tdlib-key|api-hash)")
        private const val VERSION: Byte = 1
        private const val SECRET_BYTES = 32
        private const val NONCE_BYTES = 12
        private const val RECORD_BYTES = 1 + NONCE_BYTES + SECRET_BYTES + 16
        // Serializes all instances in the process, including AtomicFile recovery.
        private val LOCK = Any()
    }

    private fun record(name: String): AtomicFile {
        require(NAME.matches(name) && !name.startsWith("00000000-0000-0000-0000-000000000000:"))
        check(directory.isDirectory || directory.mkdirs())
        check(directory.canonicalFile == directory.absoluteFile)
        val file = File(directory, name.replace(':', '_'))
        for (candidate in listOf(file, File("${file.path}.bak"), File("${file.path}.new"))) {
            check(candidate.canonicalFile == candidate.absoluteFile)
        }
        return AtomicFile(file)
    }

    private fun key(name: String, create: Boolean): SecretKey {
        val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        val alias = aliasPrefix + name
        if (store.containsAlias(alias)) {
            return store.getKey(alias, null) as SecretKey
        }
        // Never replace a missing or invalidated key for existing ciphertext.
        check(create)
        val generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore")
        generator.init(
            KeyGenParameterSpec.Builder(alias, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                .setKeySize(256)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setRandomizedEncryptionRequired(true)
                .build()
        )
        return generator.generateKey()
    }

    private fun aad(name: String) = "kiyora-secret-v1:$name".toByteArray(Charsets.UTF_8)

    fun get(name: String): ByteArray? = synchronized(LOCK) {
        val file = record(name)
        val input = try {
            file.openRead()
        } catch (error: FileNotFoundException) {
            // Missing files are distinct from unreadable/malformed storage.
            if (file.baseFile.exists() || File("${file.baseFile.path}.bak").exists()) throw error
            return@synchronized null
        }
        val bytes = input.use {
            val buffer = ByteArray(RECORD_BYTES + 1)
            var total = 0
            while (total < buffer.size) {
                val count = it.read(buffer, total, buffer.size - total)
                if (count < 0) break
                check(count > 0)
                total += count
            }
            check(total == RECORD_BYTES)
            buffer.copyOf(total)
        }
        check(bytes[0] == VERSION)
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.DECRYPT_MODE, key(name, false), GCMParameterSpec(128, bytes, 1, NONCE_BYTES))
        cipher.updateAAD(aad(name))
        cipher.doFinal(bytes, 1 + NONCE_BYTES, SECRET_BYTES + 16).also { check(it.size == SECRET_BYTES) }
    }

    fun set(name: String, value: ByteArray) = synchronized(LOCK) {
        require(value.size == SECRET_BYTES)
        val file = record(name)
        // Authenticate an existing record first. Corruption/key loss must not
        // silently become a fresh key and destroy recoverable session material.
        val previous = get(name)
        val cipher = try {
            Cipher.getInstance("AES/GCM/NoPadding").apply {
                init(Cipher.ENCRYPT_MODE, key(name, previous == null))
                updateAAD(aad(name))
            }
        } finally {
            previous?.fill(0)
        }
        check(cipher.iv.size == NONCE_BYTES)
        val ciphertext = cipher.doFinal(value)
        val output = file.startWrite()
        try {
            output.write(byteArrayOf(VERSION))
            output.write(cipher.iv)
            output.write(ciphertext)
            file.finishWrite(output)
        } catch (error: Exception) {
            file.failWrite(output)
            throw error
        }
        // AtomicFile can report I/O problems through logs instead of exceptions.
        // Verify the durable result before acknowledging success to Rust.
        val saved = get(name)
        try {
            check(saved != null && java.security.MessageDigest.isEqual(saved, value))
        } finally {
            saved?.fill(0)
        }
    }
}
