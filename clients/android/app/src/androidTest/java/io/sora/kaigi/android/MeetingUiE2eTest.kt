package io.sora.kaigi.android

import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performTextClearance
import androidx.compose.ui.test.performTextInput
import androidx.test.ext.junit.runners.AndroidJUnit4
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class MeetingUiE2eTest {
    @get:Rule
    val composeRule = createAndroidComposeRule<MainActivity>()

    @Test
    fun dashboardFlowUpdatesConfigAndOpensFallback() {
        composeRule.onNodeWithTag("kaigi.header.title").assertIsDisplayed()
        composeRule.onNodeWithTag("kaigi.status.label").assertIsDisplayed()

        composeRule.onNodeWithTag("kaigi.config.room_id").performTextClearance()
        composeRule.onNodeWithTag("kaigi.config.room_id").performTextInput("android-ui-e2e-room")

        composeRule.onNodeWithTag("kaigi.config.participant_name").performTextClearance()
        composeRule.onNodeWithTag("kaigi.config.participant_name").performTextInput("Android UI E2E")

        composeRule.onNodeWithTag("kaigi.config.require_payment_settlement").performClick()
        composeRule.onNodeWithTag("kaigi.config.require_payment_settlement").performClick()

        composeRule.onNodeWithTag("kaigi.controls.connect").performClick()
        composeRule.onNodeWithTag("kaigi.status.label").assertIsDisplayed()

        composeRule.onNodeWithTag("kaigi.session.e2ee_line").assertIsDisplayed()
    }
}
