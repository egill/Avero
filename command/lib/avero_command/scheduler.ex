defmodule AveroCommand.Scheduler do
  @moduledoc """
  Quantum scheduler for periodic jobs and reports.

  Runs scheduled tasks like:
  - Gates idle checks (every 5 minutes)
  - Hourly summaries
  - Daily summaries
  - Shift change reports
  - Traffic anomaly detection
  """

  use Quantum, otp_app: :avero_command
end
