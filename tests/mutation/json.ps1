# Reads JSON into plain PowerShell objects through System.Text.Json, for mutate.ps1 and report.ps1.
#
# NOT ConvertFrom-Json. Before PowerShell 7.5 it turns every string that looks like a date into a
# DateTime, with no switch to stop it, and the registry quotes code and calendar data in which a date
# is an ordinary string. A runner that read a mutation's text as a DateTime would write back
# something nobody wrote. Both scripts read through this one function so that they cannot disagree
# about what a file says.

function ConvertFrom-JsonElement([Text.Json.JsonElement] $Element) {
    switch ($Element.ValueKind.ToString()) {
        'Object' {
            $fields = [ordered]@{}
            foreach ($property in $Element.EnumerateObject()) {
                $fields[$property.Name] = ConvertFrom-JsonElement $property.Value
            }
            return [pscustomobject] $fields
        }
        'Array' {
            $items = [Collections.Generic.List[object]]::new()
            foreach ($item in $Element.EnumerateArray()) { $items.Add((ConvertFrom-JsonElement $item)) }
            # The comma keeps an array of one, or of none, an array once the pipeline unrolls it.
            return , $items.ToArray()
        }
        'String' { return $Element.GetString() }
        'Number' {
            $whole = 0L
            if ($Element.TryGetInt64([ref] $whole)) { return $whole }
            return $Element.GetDouble()
        }
        'True' { return $true }
        'False' { return $false }
        default { return $null }
    }
}

# A list at the root comes back one item at a time, the way any PowerShell command hands back a
# list, so a caller writes @(ConvertFrom-JsonText $text) and gets the items rather than one array
# holding them. Returning the call itself handed the array over whole (measured 2026-10-16).
function ConvertFrom-JsonText([string] $Text) {
    $document = [Text.Json.JsonDocument]::Parse($Text)
    try {
        $value = ConvertFrom-JsonElement $document.RootElement
        return $value
    }
    finally {
        $document.Dispose()
    }
}
