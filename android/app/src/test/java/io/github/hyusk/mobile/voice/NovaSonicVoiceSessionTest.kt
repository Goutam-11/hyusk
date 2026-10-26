package io.github.hyusk.mobile.voice

import org.junit.Assert.assertFalse
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class NovaSonicVoiceSessionTest {
    @Test
    fun classifiesBedrockCredentialTypesWithoutExposingValues() {
        assertEquals("iam-access-key", bedrockCredentialKind("AKIAEXAMPLE"))
        assertEquals("temporary-iam-credentials-require-session-token", bedrockCredentialKind("ASIAEXAMPLE"))
        assertEquals("bedrock-api-key-unsupported-for-bidirectional-stream", bedrockCredentialKind("ABSKEXAMPLE"))
        assertEquals("unknown-credential-format", bedrockCredentialKind("other"))
    }

    @Test
    fun recognizesOnlyHttp2CancelResetsAsExpectedStreamShutdown() {
        val reset = StreamResetException("stream was reset: CANCEL")

        assertTrue(isBedrockStreamCancellation(reset))
        assertTrue(isBedrockStreamCancellation(IllegalStateException("wrapper", reset)))
        assertFalse(isBedrockStreamCancellation(StreamResetException("stream was reset: PROTOCOL_ERROR")))
        assertFalse(isBedrockStreamCancellation(IllegalStateException("unrelated failure")))
    }

    @Test
    fun guardOnlyContainsSdkRequestBodyReset() {
        val sdkReset = StreamResetException("stream was reset: CANCEL").apply {
            stackTrace = arrayOf(StackTraceElement(
                "aws.smithy.kotlin.runtime.http.engine.okhttp.StreamingRequestBody",
                "doWriteTo", "StreamingRequestBody.kt", 64,
            ))
        }
        val appReset = StreamResetException("stream was reset: CANCEL").apply {
            stackTrace = arrayOf(StackTraceElement("io.github.hyusk.mobile.MainActivity", "onCreate", "MainActivity.kt", 1))
        }

        assertTrue(isUncaughtBedrockWriterReset(sdkReset))
        assertFalse(isUncaughtBedrockWriterReset(appReset))
        assertFalse(isUncaughtBedrockWriterReset(IllegalStateException("unrelated")))
    }

    private class StreamResetException(message: String) : Exception(message)
}
