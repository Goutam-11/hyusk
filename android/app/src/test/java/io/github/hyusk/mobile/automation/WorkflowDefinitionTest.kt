package io.github.hyusk.mobile.automation

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class WorkflowDefinitionTest {
    @Test
    fun validateAllowsOnlyKnownTargets() {
        validateWorkflowTarget("phone", stepNumber = 1)
        validateWorkflowTarget("laptop", stepNumber = 1)

        val failure = runCatching { validateWorkflowTarget("desktop", stepNumber = 1) }.exceptionOrNull()
        assertTrue(failure is IllegalArgumentException)
        assertTrue(failure?.message.orEmpty().contains("not supported: desktop"))
    }

    @Test
    fun localExecutionRejectsLaptopTargetBeforeRunningAnySteps() {
        val failure = runCatching { validateLocalWorkflowTarget("laptop", stepNumber = 2) }.exceptionOrNull()
        assertTrue(failure is IllegalArgumentException)
        assertTrue(failure?.message.orEmpty().contains("Step 2 targets the laptop"))
        assertTrue(failure?.message.orEmpty().contains("cannot route laptop actions"))
    }

    @Test
    fun localExecutionRejectsUnknownTarget() {
        val failure = runCatching { validateLocalWorkflowTarget("desktop", stepNumber = 3) }.exceptionOrNull()
        assertTrue(failure is IllegalArgumentException)
        assertTrue(failure?.message.orEmpty().contains("Step 3 has unsupported local target: desktop"))
    }

    @Test
    fun continueOnErrorSummaryCountsFailureSeparatelyFromSuccess() {
        val summary = workflowRunSummary(
            listOf(
                WorkflowStepOutcome(1, "apps.launch", true, "Opened YouTube"),
                WorkflowStepOutcome(2, "url.open", false, "No app can handle that action"),
                WorkflowStepOutcome(3, "media.play", true, "Play control sent"),
            ),
            totalSteps = 3,
        )

        assertEquals(
            "Partial completion: 2/3 steps succeeded; 1 failed. Step 2 (url.open) failed: No app can handle that action",
            summary,
        )
    }

    @Test
    fun stoppedWorkflowReportsUnrunSteps() {
        val summary = workflowRunSummary(
            listOf(WorkflowStepOutcome(1, "apps.launch", false, "App not found")),
            totalSteps = 3,
        )

        assertTrue(summary.contains("0/3 steps succeeded; 1 failed; 2 not run"))
        assertTrue(summary.contains("Step 1 (apps.launch) failed: App not found"))
    }
}
