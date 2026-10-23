package io.github.arkfarin.kiyora.nativebridge

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import java.io.File
import java.security.KeyStore
import java.util.UUID
import org.junit.After
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith

/** Uses real AndroidKeyStore. No Telegram/network access or production aliases. */
@RunWith(AndroidJUnit4::class)
class KeystoreSecretsTest {
    private val context = InstrumentationRegistry.getInstrumentation().targetContext
    private val prefix = "kiyora.instrumentation.${UUID.randomUUID()}."
    private val directory = File(context.noBackupFilesDir.canonicalFile, prefix)
    private val name = "${UUID.randomUUID()}:tdlib-key"
    private val other = "${UUID.randomUUID()}:tdlib-key"
    private fun store() = KeystoreSecrets(context, directory, prefix)
    private fun record(key: String) = File(directory, key.replace(':', '_'))

    @After
    fun cleanup() {
        val keys = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        keys.aliases().toList().filter { it.startsWith(prefix) }.forEach { keys.deleteEntry(it) }
        directory.deleteRecursively()
        File(context.noBackupFilesDir.canonicalFile, "kiyora-secrets-v1").listFiles()
            ?.filter { it.name == name.replace(':', '_') }?.forEach { it.delete() }
    }

    @Test
    fun persistsAcrossInstancesAndRandomizesCiphertext() {
        val value = ByteArray(32) { it.toByte() }
        assertNull(store().get(name))
        store().set(name, value)
        val first = record(name).readBytes()
        assertArrayEquals(value, store().get(name))
        assertFalse(first.toList().windowed(value.size).any { it == value.toList() })
        store().set(name, value)
        assertFalse(first.contentEquals(record(name).readBytes()))
        assertArrayEquals(value, store().get(name))
    }

    @Test
    fun swappedOrCorruptRecordsFailWithoutOverwriting() {
        store().set(name, ByteArray(32) { 1 })
        store().set(other, ByteArray(32) { 2 })
        val original = record(name).readBytes()
        record(other).writeBytes(original)
        assertThrows(Exception::class.java) { store().get(other) }
        assertThrows(Exception::class.java) { store().set(other, ByteArray(32) { 3 }) }
        assertArrayEquals(original, record(other).readBytes())
        original[original.lastIndex] = (original.last().toInt() xor 1).toByte()
        record(name).writeBytes(original)
        assertThrows(Exception::class.java) { store().get(name) }
        assertThrows(Exception::class.java) { store().set(name, ByteArray(32)) }
        assertArrayEquals(original, record(name).readBytes())
    }

    @Test
    fun missingKeystoreKeyNeverResetsExistingCiphertext() {
        store().set(name, ByteArray(32) { 4 })
        val bytes = record(name).readBytes()
        KeyStore.getInstance("AndroidKeyStore").apply { load(null); deleteEntry(prefix + name) }
        assertThrows(Exception::class.java) { store().get(name) }
        assertThrows(Exception::class.java) { store().set(name, ByteArray(32) { 5 }) }
        assertArrayEquals(bytes, record(name).readBytes())
    }

    @Test
    fun validatesNamesAndBoundsBeforeUsingKeystore() {
        for (bad in listOf("../escape", "${UUID.randomUUID()}:unknown", "00000000-0000-0000-0000-000000000000:tdlib-key")) {
            assertThrows(Exception::class.java) { store().set(bad, ByteArray(32)) }
        }
        assertThrows(Exception::class.java) { store().set(name, ByteArray(31)) }
        store().set(name, ByteArray(32))
        record(name).appendBytes(ByteArray(1024))
        assertThrows(Exception::class.java) { store().get(name) }
    }

    @Test
    fun atomicFileRecoversBackupAfterInterruptedReplacement() {
        val value = ByteArray(32) { 7 }
        store().set(name, value)
        val file = record(name)
        check(file.renameTo(File("${file.path}.bak")))
        file.writeBytes(byteArrayOf(0))
        assertArrayEquals(value, store().get(name))
    }

    @Test
    fun frameworkDirectoryAliasWorksButRecordSymlinksAreRejected() {
        val defaultStore = KeystoreSecrets(context, aliasPrefix = prefix)
        val value = ByteArray(32) { 9 }
        defaultStore.set(name, value)
        assertArrayEquals(value, defaultStore.get(name))

        check(directory.mkdirs())
        val target = File(directory, "outside-record").apply { writeBytes(byteArrayOf(42)) }
        android.system.Os.symlink(target.path, record(name).path)
        assertThrows(Exception::class.java) { store().get(name) }
        assertThrows(Exception::class.java) { store().set(name, value) }
        assertArrayEquals(byteArrayOf(42), target.readBytes())
    }
}
