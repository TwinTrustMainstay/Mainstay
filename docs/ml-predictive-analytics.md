# Predictive Analytics (`train_model`, `predict_model`, `rank_engineers`)

Closes #1758.

## What runs on-chain

Contracts cannot train large models or run arbitrary ML code. The on-chain
part is a **linear model** with integer fixed-point arithmetic. It covers:

| Task | Function |
|------|----------|
| Train failure prediction, maintenance scheduling, engineer ranking | `train_model(admin, model, features, targets, epochs, lr)` |
| Model serving API | `predict_model(model, features)`, `rank_engineers(model, candidates, features)` |
| Accuracy and drift | `record_model_outcome(admin, model, features, actual)`, `get_model_metrics(model)` |

Use one model name per task, for example `failure`, `schedule`, `engineer_rank`.

## Model

`prediction (milli-units) = bias + sum(weights[i] * features[i])`

- Weights, bias, predictions, and errors are in milli-units (1000 = 1.0).
- Features and targets are integers with `|value| <= 1,000,000`.
- Learning rate is per sample, in milli-units, `(0, 1,000,000]`.
- Weights and bias are clamped to `+/- 1e9` milli-units.
- Up to 8 features, 100 samples per call, and 20 epochs per call.

Feature engineering (turning asset and engineer records into integers) happens
off-chain. The contract stores only the trained weights.

## Accuracy and drift

- Each `train_model` call records a **baseline error**: the mean absolute error
  on the training batch.
- `record_model_outcome` records each observed outcome. It tracks the mean
  absolute error and an exponential moving average (alpha = 1/8).
- **Drift** is flagged when, after at least 8 outcomes, the recent error exceeds
  `2 x baseline + 1 unit`. Retraining resets the flag.

## Limitations

- A linear model is far simpler than a neural network or gradient-boosted
  trees. Its accuracy depends on how well the features fit a linear relation.
- Training is online SGD inside the contract. Gas use scales with
  `samples x epochs x features`, which is why the limits above exist. Large
  training runs should happen off-chain, with the resulting weights submitted
  through a separate admin-only path.
- The model is trusted: the admin sets the training data. Any manipulation of
  training data is the admin's responsibility.
