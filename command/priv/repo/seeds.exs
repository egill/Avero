# Script for populating the database.
#
# You can run it as:
#     mix run priv/repo/seeds.exs

alias AveroCommand.Repo
alias AveroCommand.Store.SiteConfig

# Insert default site configs if they don't exist
sites = [
  %{site: "AVERO-HQ", gateway_id: "AP-AVERO-GR-01", name: "Avero HQ Test Site", timezone: "Atlantic/Reykjavik"},
  %{site: "NETTO-GRANDI", gateway_id: "AP-NETTO-GR-01", name: "Netto Grandi Production", timezone: "Atlantic/Reykjavik"},
  %{site: "docker-test", gateway_id: "dev-gateway-01", name: "Docker Test Site", timezone: "UTC"}
]

Enum.each(sites, fn site_attrs ->
  case Repo.get(SiteConfig, site_attrs.site) do
    nil ->
      %SiteConfig{}
      |> SiteConfig.changeset(site_attrs)
      |> Repo.insert!()
      IO.puts("Created site config: #{site_attrs.site}")

    _existing ->
      IO.puts("Site config already exists: #{site_attrs.site}")
  end
end)
