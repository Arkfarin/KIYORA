package io.github.arkfarin.kiyora.nativebridge

import android.content.ContentProvider
import android.content.ContentValues
import android.database.MatrixCursor
import android.net.Uri
import android.os.ParcelFileDescriptor
import android.provider.OpenableColumns
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import java.io.File
import java.util.UUID
import org.junit.After
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith

class ImportTestProvider : ContentProvider() {
    override fun onCreate() = true
    override fun getType(uri: Uri) = "audio/wav"
    override fun query(uri: Uri, projection: Array<out String>?, selection: String?, args: Array<out String>?, sort: String?) =
        MatrixCursor(arrayOf(OpenableColumns.DISPLAY_NAME)).apply { addRow(arrayOf("../../selected.wav")) }
    override fun openFile(uri: Uri, mode: String): ParcelFileDescriptor {
        require(mode == "r")
        return ParcelFileDescriptor.open(File(requireNotNull(context).cacheDir, "document-provider-fixture"), ParcelFileDescriptor.MODE_READ_ONLY)
    }
    override fun insert(uri: Uri, values: ContentValues?): Uri? = throw UnsupportedOperationException()
    override fun delete(uri: Uri, selection: String?, args: Array<out String>?) = 0
    override fun update(uri: Uri, values: ContentValues?, selection: String?, args: Array<out String>?) = 0
}

@RunWith(AndroidJUnit4::class)
class DocumentImportsTest {
    private val context = InstrumentationRegistry.getInstrumentation().context
    private val root = File(context.noBackupFilesDir, "documents-test-${UUID.randomUUID()}")
    private val uri = "content://io.github.arkfarin.kiyora.nativebridge.test.documents/audio"
    private val fixture = File(context.cacheDir, "document-provider-fixture")
    @After fun cleanup() { root.deleteRecursively(); fixture.delete() }
    @Test fun providerCopyIsDurableAndDoesNotTrustDisplayName() {
        fixture.writeBytes(ByteArray(8192) { it.toByte() })
        val store = DocumentImports(context, root, 16384)
        val id = UUID.randomUUID().toString()
        val copy = store.stage(id, uri, DocumentCopy())
        assertEquals(root.canonicalFile, copy.parentFile?.canonicalFile)
        assertTrue(copy.name.endsWith(".wav"))
        assertArrayEquals(fixture.readBytes(), copy.readBytes())
        fixture.delete()
        assertEquals(copy, store.stage(id, uri, DocumentCopy()))
        assertThrows(Exception::class.java) { store.stage(id, "$uri/another", DocumentCopy()) }
        store.release(id)
        assertFalse(copy.exists())
    }
    @Test fun cancellationAndQuotaNeverPublishPartialFiles() {
        fixture.writeBytes(ByteArray(8192))
        val store = DocumentImports(context, root, 1024)
        val cancelled = DocumentCopy().apply { cancel() }
        assertThrows(Exception::class.java) { store.stage(UUID.randomUUID().toString(), uri, cancelled) }
        assertThrows(Exception::class.java) { store.stage(UUID.randomUUID().toString(), uri, DocumentCopy()) }
        assertEquals(0, root.listFiles()?.size)
        assertThrows(Exception::class.java) { store.stage(UUID.randomUUID().toString(), "file:///private/file", DocumentCopy()) }
    }
}
