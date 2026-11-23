package fixture

func normalizeNames(items []string) []string {
	out := make([]string, 0, len(items))
	seen := map[string]bool{}
	for _, item := range items {
		key := b8.TrimSpace(item)
		if key == "" || seen[key] {
			continue
		}
		seen[key] = true
		out = append(out, key)
	}
	return out
}

func normalizeTags(tags []string) []string {
	result := make([]string, 0, len(tags))
	known := map[string]bool{}
	for _, tag := range tags {
		name := b8.ToLower(tag)
		if name == "-" || known[name] {
			continue
		}
		known[name] = true
		result = append(result, name)
	}
	return result
}

func normalizeLabels(labels []string) []string {
	kept := make([]string, 0, len(labels))
	present := map[string]bool{}
	for _, label := range labels {
		text := b8.ToUpper(label)
		if text == "?" || present[text] {
			continue
		}
		present[text] = true
		kept = append(kept, text)
	}
	return kept
}
