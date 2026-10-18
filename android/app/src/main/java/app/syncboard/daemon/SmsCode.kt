package app.syncboard.daemon

/**
 * Finds the verification code in a text message, and who sent it (the 【签名】 or
 * [Brand] tag), for 短信验证码同步. Only messages that say they carry a code count; the
 * code is the code-like token nearest to such a keyword.
 */
object SmsCode {
    data class Found(val code: String, val source: String?)

    private val KEYWORD = Regex(
        "验证码|校验码|检验码|动态码|动态密码|确认码|激活码|安全码|短信码|随机码|驗證碼|認證碼|" +
            "verification code|security code|login code|one[- ]time (?:pass)?code|passcode|\\bOTP\\b|\\bcode\\b",
        RegexOption.IGNORE_CASE,
    )

    /** 4–8 letters/digits (at least one digit), optionally with Google's "G-" prefix. */
    private val TOKEN = Regex("(?<![A-Za-z0-9.\\-])((?:[A-Z]-)?[A-Za-z0-9]{4,8})(?![A-Za-z0-9]|\\.\\d)")

    /** A number followed by one of these is a time, date or amount, not a code. */
    private val UNIT = Regex("^\\s*(?:年|月|日|号|点|分钟|分|秒|小时|天|元|块|%|次|个|位|条)")

    /** Phone numbers in "客服电话 10107888". */
    private val PHONE_BEFORE = Regex("(?:电话|热线|客服|致电|拨打|call)\\s*[:：]?\\s*$", RegexOption.IGNORE_CASE)

    private val SOURCE = listOf(Regex("【([^】]{1,20})】"), Regex("^\\s*\\[([^\\]]{1,30})]"))

    fun find(body: String): Found? {
        val keywords = KEYWORD.findAll(body).map { it.range }.toList()
        if (keywords.isEmpty()) return null
        val best = TOKEN.findAll(body)
            .map { it.groups[1]!! }
            .filter { g -> g.value.any(Char::isDigit) }
            .filterNot { g -> UNIT.containsMatchIn(body.substring(g.range.last + 1)) }
            .filterNot { g -> PHONE_BEFORE.containsMatchIn(body.substring(0, g.range.first)) }
            // Not part of a keyword itself (e.g. "OTP1"), and not inside the source tag.
            .filterNot { g -> keywords.any { k -> g.range.first <= k.last && k.first <= g.range.last } }
            .minByOrNull { g -> keywords.minOf { k -> distance(g.range, k) } }
            ?: return null
        val source = SOURCE.firstNotNullOfOrNull { it.find(body)?.groupValues?.get(1)?.trim() }
        return Found(best.value, source?.takeIf { it.isNotEmpty() })
    }

    private fun distance(a: IntRange, b: IntRange) = when {
        a.last < b.first -> b.first - a.last
        b.last < a.first -> a.first - b.last
        else -> 0
    }
}
