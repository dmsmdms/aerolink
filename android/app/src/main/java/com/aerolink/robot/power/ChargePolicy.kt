package com.aerolink.robot.power

/**
 * Phone battery window (docs/04-power.md PWR-020/021): charge up to 60 %, resume at 45 %,
 * never charge above 40 °C; after heat, resume only below 37 °C (and at ≤ 45 %).
 */
object ChargePolicy {
    const val STOP_LEVEL = 60
    const val RESUME_LEVEL = 45
    const val HOT_DECI_C = 400
    const val COOL_DECI_C = 370

    /** Decision for the first battery reading, before any switching happened. */
    fun initial(levelPercent: Int, tempDeciC: Int): Boolean =
        levelPercent < STOP_LEVEL && tempDeciC <= HOT_DECI_C

    fun next(chargingAllowed: Boolean, levelPercent: Int, tempDeciC: Int): Boolean = when {
        tempDeciC > HOT_DECI_C -> false
        chargingAllowed -> levelPercent < STOP_LEVEL
        else -> levelPercent <= RESUME_LEVEL && tempDeciC < COOL_DECI_C
    }
}
