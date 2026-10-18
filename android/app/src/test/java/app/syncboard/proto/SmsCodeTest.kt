package app.syncboard.proto

import app.syncboard.daemon.SmsCode
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class SmsCodeTest {
    private fun code(s: String) = SmsCode.find(s)?.code
    private fun source(s: String) = SmsCode.find(s)?.source

    @Test fun typicalChinese() {
        assertEquals("482913", code("【支付宝】您的验证码是 482913，5分钟内有效，请勿泄露给他人。"))
        assertEquals("支付宝", source("【支付宝】您的验证码是 482913，5分钟内有效，请勿泄露给他人。"))
        assertEquals("6391", code("验证码6391，用于登录12306，10分钟内有效。"))
        assertEquals("208416", code("【京东】208416（动态码），您正在登录，切勿告知他人。"))
        assertEquals("73915", code("您的校验码为：73915。如非本人操作请忽略【中国银行】"))
        assertEquals("中国银行", source("您的校验码为：73915。如非本人操作请忽略【中国银行】"))
    }

    @Test fun codeBeforeKeyword() {
        assertEquals("552019", code("552019 是你的微信验证码，请在 10 分钟内输入。"))
    }

    @Test fun alphanumeric() {
        assertEquals("A7K29Q", code("[GitHub] Your verification code is A7K29Q."))
        assertEquals("GitHub", source("[GitHub] Your verification code is A7K29Q."))
        assertEquals("G-381927", code("G-381927 is your Google verification code."))
    }

    @Test fun english() {
        assertEquals("190284", code("Your OTP is 190284. Do not share it with anyone."))
        assertEquals("8812", code("Use code 8812 to sign in to Steam"))
    }

    @Test fun notACode() {
        assertNull(code("【顺丰速运】您的快递已到达菜鸟驿站，取件码 3-2-1018，请于今日 18:00 前取件。"))
        assertNull(code("您本月话费 128.50 元，余额 23.10 元。"))
        assertNull(code("周五下午 3 点在 302 开会。"))
        assertNull(code("验证码功能已上线"))
    }

    @Test fun skipsNumbersThatAreNotTheCode() {
        // Minutes, years and phone numbers around the code.
        assertEquals("739104", code("【美团】2026年您的验证码为739104，有效期15分钟，客服电话10107888。"))
        assertEquals("4417", code("尊敬的用户，您的短信验证码为4417，有效期为5分钟。"))
    }
}
