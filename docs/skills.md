# Skills

Agent skills used in this repo, with provenance. Update this file whenever a skill is added, removed or refreshed.

## Native skills (`.claude/skills/`, always loaded)

| Skill                         | Source                                                                                             | Revision     | License        | Notes                                                       |
| ----------------------------- | -------------------------------------------------------------------------------------------------- | ------------ | -------------- | ----------------------------------------------------------- |
| `use-otherskill`              | OtherSkill companion (installed by `otherskill setup`, symlink to `.agents/skills/use-otherskill`) | n/a          | see OtherSkill | How to use the bank CLI                                     |
| `subagent-driven-development` | Adapted from `superorchestrator/.claude/skills/subagent-driven-development`                        | n/a          | project        | Native subagent flow is primary; superorchestrator optional |
| `test-driven-development`     | https://github.com/obra/superpowers/tree/HEAD/skills/test-driven-development                       | 22f0b463e7a5 | MIT            | Unmodified, LICENSE included                                |
| `skill-creator`               | https://github.com/anthropics/skills/tree/HEAD/skills/skill-creator                                | ca45fc4      | Apache-2.0     | Unmodified, LICENSE.txt included                            |
| `find-skills`                 | https://github.com/vercel-labs/skills/tree/HEAD/skills/find-skills                                 | a0a1bb9      | MIT            | Unmodified, LICENSE included                                |

## OtherSkill bank (`.otherskill/bank/`, routed per prompt)

The bank is invisible to native skill discovery. The `UserPromptSubmit` hook in `.claude/settings.json` runs `otherskill find`, which needs `OPENROUTER_API_KEY` (free model) and injects only the matching skill names. Without a key the hook does nothing and prompts are unaffected.

| Category         | Skill                            | Source                                                                                                     | Revision     | License          |
| ---------------- | -------------------------------- | ---------------------------------------------------------------------------------------------------------- | ------------ | ---------------- |
| `backend/rust`   | `domain-iot`                     | https://github.com/actionbook/rust-skills/tree/HEAD/skills/domain-iot                                      | 5c40d3ad7851 | MIT (per README) |
| `backend/rust`   | `m06-error-handling`             | https://github.com/actionbook/rust-skills/tree/HEAD/skills/m06-error-handling                              | 5c40d3ad7851 | MIT (per README) |
| `backend/rust`   | `m07-concurrency`                | https://github.com/actionbook/rust-skills/tree/HEAD/skills/m07-concurrency                                 | 5c40d3ad7851 | MIT (per README) |
| `backend/rust`   | `rust-best-practices`            | https://github.com/apollographql/skills/tree/HEAD/skills/rust-best-practices                               | 9dda13977303 | MIT              |
| `devops`         | `api-gateway`                    | https://github.com/awslabs/agent-plugins/tree/HEAD/plugins/aws-serverless/skills/api-gateway               | fbefb3764e67 | Apache-2.0       |
| `devops`         | `aws-lambda`                     | https://github.com/awslabs/agent-plugins/tree/HEAD/plugins/aws-serverless/skills/aws-lambda                | fbefb3764e67 | Apache-2.0       |
| `devops`         | `aws-serverless-deployment`      | https://github.com/awslabs/agent-plugins/tree/HEAD/plugins/aws-serverless/skills/aws-serverless-deployment | fbefb3764e67 | Apache-2.0       |
| `docs`           | `agents-md`                      | https://github.com/getsentry/skills/tree/HEAD/skills/agents-md                                             | 0c464f411bdf | Apache-2.0       |
| `frontend/react` | `expo-data-fetching`             | https://github.com/expo/skills/tree/HEAD/plugins/expo/skills/expo-data-fetching                            | a9e5bd3fa00b | MIT              |
| `frontend/react` | `expo-project-structure`         | https://github.com/expo/skills/tree/HEAD/plugins/expo/skills/expo-project-structure                        | a9e5bd3fa00b | MIT              |
| `frontend/react` | `expo-router`                    | https://github.com/expo/skills/tree/HEAD/plugins/expo/skills/expo-router                                   | a9e5bd3fa00b | MIT              |
| `frontend/ui-ux` | `expo-design-system`             | https://github.com/expo/skills/tree/HEAD/plugins/expo/skills/expo-design-system                            | a9e5bd3fa00b | MIT              |
| `frontend/ui-ux` | `expo-native-ui`                 | https://github.com/expo/skills/tree/HEAD/plugins/expo/skills/expo-native-ui                                | a9e5bd3fa00b | MIT              |
| `frontend/ui-ux` | `frontend-design`                | https://github.com/anthropics/skills/tree/HEAD/skills/frontend-design                                      | ca45fc496a34 | Apache-2.0       |
| `management`     | `finishing-a-development-branch` | https://github.com/obra/superpowers/tree/HEAD/skills/finishing-a-development-branch                        | 22f0b463e7a5 | MIT              |
| `management`     | `writing-plans`                  | https://github.com/obra/superpowers/tree/HEAD/skills/writing-plans                                         | 22f0b463e7a5 | MIT              |
| `security`       | `gha-security-review`            | https://github.com/getsentry/skills/tree/HEAD/skills/gha-security-review                                   | 0c464f411bdf | Apache-2.0       |
| `security`       | `security-review`                | https://github.com/getsentry/skills/tree/HEAD/skills/security-review                                       | 0c464f411bdf | Apache-2.0       |
| `testing`        | `playwright-cli`                 | https://github.com/microsoft/playwright-cli/tree/HEAD/skills/playwright-cli                                | f50f57905b9f | Apache-2.0       |
| `testing`        | `systematic-debugging`           | https://github.com/obra/superpowers/tree/HEAD/skills/systematic-debugging                                  | 22f0b463e7a5 | MIT              |
| `testing`        | `verification-before-completion` | https://github.com/obra/superpowers/tree/HEAD/skills/verification-before-completion                        | 22f0b463e7a5 | MIT              |
| `testing`        | `webapp-testing`                 | https://github.com/anthropics/skills/tree/HEAD/skills/webapp-testing                                       | ca45fc496a34 | Apache-2.0       |

22 bank skills, 5 native skills.

## Rejected or skipped

- `trailofbits/skills`: CC-BY-SA-4.0 (share-alike), not suitable to vendor.
- `vercel-labs/agent-skills` (React Native, web design): no license file in the repository.
- `mobile-dev-inc/Maestro`: its skills are release tooling for Maestro itself, not test-authoring guidance.
- GitHub code search (`otherskill search`) found no usable skill for DynamoDB single-table design, AWS IoT Core, Raspberry Pi/CUPS printing, payment webhooks or AWS CDK. Those areas rely on the AWS MCP servers in `.mcp.json` and on `docs/contracts.md`. Add skills later with `otherskill add`.

## Hook setup

`otherskill setup --non-interactive --agent claude --scope project --portable` wrote the project hook into `.claude/settings.json`. It uses `$HOME/.local/share/otherskill/bin/otherskill` and `$CLAUDE_PROJECT_DIR/otherskill.config.json`, so nothing machine specific is committed. Each machine needs the launcher once:

```sh
git clone https://github.com/datamonsterr/other-skill && cd other-skill
bun install --frozen-lockfile && bun otherskill.ts setup --non-interactive --agent claude --scope global
export OPENROUTER_API_KEY=...   # free OpenRouter key
otherskill doctor
```

## Adding a skill

```sh
otherskill add github:<owner>/<repo> --path <dir> --category <id> --scope project
```

Inspect `SKILL.md` and the license first, then add a row above. Keep the bank near 25 skills.
