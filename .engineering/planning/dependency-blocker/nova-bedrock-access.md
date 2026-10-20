---
format: aep.planning-md/3
id: dependency-blocker:nova-bedrock-access
kind: dependency-blocker
status: open
title: The operator's AWS role cannot invoke Nova Sonic in a region that offers it
relations:
- blocks: story:nova-sonic-client
- blocks: story:nova-sonic-adapter
revision: 1
---
## What is blocked

The live acceptance of `story:nova-sonic-client` and `story:nova-sonic-adapter`: one audio-in,
audio-out exchange with Amazon Nova Sonic on Bedrock.

## What was observed (2026-10-20, live tests run from the operator's machine)

- In the operator's default region, eu-central-1, Bedrock answers `ValidationException: The
  provided model identifier is invalid.` for both `amazon.nova-2-sonic-v1:0` and
  `amazon.nova-sonic-v1:0`: the model is not offered there (the model card lists us-east-1,
  us-west-2, eu-north-1 and ap-northeast-1).
- In eu-north-1 the request reaches the model and is refused with `AccessDeniedException`: the
  operator's role has no identity-based policy allowing `bedrock:InvokeModel` on the model.

## Clears when

The role used for the live run may invoke a Nova Sonic model in a region that offers it (an IAM
policy allowing `bedrock:InvokeModel` and `bedrock:InvokeModelWithBidirectionalStream` on that
model), or another AWS account with that access is named. Then the live tests run with
`AWS_REGION=<that region>`.
