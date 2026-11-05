#!/usr/bin/env python3
"""Self-check for enforce-subagent-models.py. Run: python3 .claude/hooks/test_enforce_subagent_models.py"""
import importlib.util
import json
import pathlib
import unittest

spec = importlib.util.spec_from_file_location(
    "guard", pathlib.Path(__file__).with_name("enforce-subagent-models.py"))
guard = importlib.util.module_from_spec(spec)
spec.loader.exec_module(guard)


def call(tool_input):
    return guard.decide(json.dumps({"tool_input": tool_input}))


class GuardTest(unittest.TestCase):
    def test_allows_project_agents(self):
        self.assertIsNone(call({"subagent_type": "implementer-sonnet"}))
        self.assertIsNone(call({"subagent_type": "spec-reviewer", "model": "haiku", "effort": "medium"}))

    def test_denies_bad_model_effort_and_general_purpose(self):
        self.assertIn("Model", call({"subagent_type": "x", "model": "opus"}))
        self.assertIn("Effort", call({"subagent_type": "x", "effort": "high"}))
        self.assertIn("general-purpose", call({}))
        self.assertIn("general-purpose", call({"subagent_type": "general-purpose"}))

    def test_fails_closed_on_malformed_input(self):
        self.assertIn("not valid JSON", guard.decide("{nope"))
        self.assertIn("not a JSON object", guard.decide("[]"))
        self.assertIn("tool_input", guard.decide('{"tool_input": 5}'))
        self.assertIn("'model' must be a string", call({"subagent_type": "x", "model": ["opus"]}))
        self.assertIn("'effort' must be a string", call({"subagent_type": "x", "effort": 1}))
        self.assertIn("'subagent_type' must be a string", call({"subagent_type": 3}))

    def test_missing_tool_input_means_general_purpose_denied(self):
        self.assertIn("general-purpose", guard.decide("{}"))


if __name__ == "__main__":
    unittest.main()
