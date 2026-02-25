package dev.yourconnector.mobile

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/**
 * Android secure storage bridge for Rust JNI calls.
 *
 * Storage model:
 * - AES key is generated and kept in Android Keystore.
 * - Cipher text is persisted in SharedPreferences.
 */
object SecureStoreBridge {
  private const val STORE_NAME = "yc_secure_store_v1"
  private const val MASTER_KEY_ALIAS = "dev.yourconnector.mobile.master"
  private const val CIPHER_TRANSFORMATION = "AES/GCM/NoPadding"
  private const val KEYSTORE_PROVIDER = "AndroidKeyStore"
  private const val KEY_SIZE_BITS = 256
  private const val GCM_TAG_BITS = 128
  private const val GCM_IV_SIZE_BYTES = 12

  private fun prefKey(service: String, account: String): String = "$service::$account"

  @JvmStatic
  fun get(context: Context, service: String, account: String): ByteArray? {
    val prefs = context.applicationContext.getSharedPreferences(STORE_NAME, Context.MODE_PRIVATE)
    val encoded = prefs.getString(prefKey(service, account), null) ?: return null
    return try {
      val payload = Base64.decode(encoded, Base64.NO_WRAP)
      if (payload.size <= GCM_IV_SIZE_BYTES) {
        null
      } else {
        val iv = payload.copyOfRange(0, GCM_IV_SIZE_BYTES)
        val cipherText = payload.copyOfRange(GCM_IV_SIZE_BYTES, payload.size)
        val cipher = Cipher.getInstance(CIPHER_TRANSFORMATION)
        cipher.init(Cipher.DECRYPT_MODE, getOrCreateSecretKey(), GCMParameterSpec(GCM_TAG_BITS, iv))
        cipher.doFinal(cipherText)
      }
    } catch (_: Exception) {
      null
    }
  }

  @JvmStatic
  fun set(context: Context, service: String, account: String, value: ByteArray): String? {
    return try {
      val cipher = Cipher.getInstance(CIPHER_TRANSFORMATION)
      cipher.init(Cipher.ENCRYPT_MODE, getOrCreateSecretKey())
      val iv = cipher.iv ?: return "cipher iv is null"
      val cipherText = cipher.doFinal(value)
      val payload = ByteArray(iv.size + cipherText.size)
      System.arraycopy(iv, 0, payload, 0, iv.size)
      System.arraycopy(cipherText, 0, payload, iv.size, cipherText.size)
      val encoded = Base64.encodeToString(payload, Base64.NO_WRAP)

      val prefs = context.applicationContext.getSharedPreferences(STORE_NAME, Context.MODE_PRIVATE)
      val saved = prefs.edit().putString(prefKey(service, account), encoded).commit()
      if (saved) {
        null
      } else {
        "shared preferences commit failed"
      }
    } catch (error: Exception) {
      "${error.javaClass.simpleName}: ${error.message ?: "unknown"}"
    }
  }

  @JvmStatic
  fun delete(context: Context, service: String, account: String): String? {
    return try {
      val prefs = context.applicationContext.getSharedPreferences(STORE_NAME, Context.MODE_PRIVATE)
      val removed = prefs.edit().remove(prefKey(service, account)).commit()
      if (removed) {
        null
      } else {
        "shared preferences remove failed"
      }
    } catch (error: Exception) {
      "${error.javaClass.simpleName}: ${error.message ?: "unknown"}"
    }
  }

  private fun getOrCreateSecretKey(): SecretKey {
    val keyStore = KeyStore.getInstance(KEYSTORE_PROVIDER)
    keyStore.load(null)
    val existing = keyStore.getKey(MASTER_KEY_ALIAS, null)
    if (existing is SecretKey) {
      return existing
    }

    val keyGenerator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, KEYSTORE_PROVIDER)
    val spec = KeyGenParameterSpec.Builder(
      MASTER_KEY_ALIAS,
      KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT
    )
      .setKeySize(KEY_SIZE_BITS)
      .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
      .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
      .setRandomizedEncryptionRequired(true)
      .setUserAuthenticationRequired(false)
      .build()
    keyGenerator.init(spec)
    return keyGenerator.generateKey()
  }
}
