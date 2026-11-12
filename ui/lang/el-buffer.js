"use strict";
// Greek, part 4: where the replay buffer is kept (Capture page, status bar, Keybinds page).
glintAddLanguage("el", "Ελληνικά", {
  "Replay buffer": "Buffer επανάληψης",
  "Keep the buffer in": "Αποθήκευση του buffer σε",
  "RAM + disk": "RAM + δίσκος",
  "RAM only": "Μόνο RAM",
  "Disk only": "Μόνο δίσκος",
  "Fastest, and nothing is written to disk. Uses up to {ram} of RAM": "Το πιο γρήγορο· δεν γράφεται τίποτα στον δίσκο. Χρησιμοποιεί έως {ram} RAM",
  "Lowest RAM use: the whole buffer is kept in files. Up to {disk} on disk": "Η μικρότερη χρήση RAM: όλο το buffer φυλάσσεται σε αρχεία. Έως {disk} στον δίσκο",
  "Your longest clip is short enough to stay in RAM, so nothing is written to disk. Up to {ram} of RAM": "Το μεγαλύτερο κλιπ σας χωράει στη RAM, οπότε δεν γράφεται τίποτα στον δίσκο. Έως {ram} RAM",
  "The newest {secs} stay in RAM and the rest is kept in files. Up to {ram} of RAM and {disk} on disk": "Τα τελευταία {secs} μένουν στη RAM και τα υπόλοιπα φυλάσσονται σε αρχεία. Έως {ram} RAM και {disk} στον δίσκο",
  "Kept in RAM": "Διάρκεια στη RAM",
  "Clips up to this length are saved straight from RAM": "Κλιπ έως αυτή τη διάρκεια αποθηκεύονται απευθείας από τη RAM",
  "Buffer folder": "Φάκελος buffer",
  "Temporary files, deleted as the buffer moves on. An SSD is best": "Προσωρινά αρχεία που διαγράφονται καθώς προχωρά το buffer. Προτιμήστε SSD",
  "Default": "Προεπιλογή",
  "Buffer <b>{now}</b> / {max} · {size} RAM + {disk} disk": "Buffer <b>{now}</b> / {max} · {size} RAM + {disk} δίσκος",
  "Fixed. Moving pictures use all of it; the {length} buffer can reach {size}": "Σταθερός. Οι κινούμενες εικόνες τον χρησιμοποιούν όλο· το buffer των {length} μπορεί να φτάσει τα {size}",
  "Buffer length follows the longest clip: {length} · at most {size}, usually far less": "Η διάρκεια του buffer ακολουθεί το μεγαλύτερο κλιπ: {length} · το πολύ {size}, συνήθως πολύ λιγότερο",

  // Settings page: search box, language section, compact overlay
  "Search settings": "Αναζήτηση ρυθμίσεων",
  "No setting matches “{text}”": "Καμία ρύθμιση δεν ταιριάζει με “{text}”",
  "Used in Glint's window, the clip overlay and the tray menu. The tray menu changes the next time Glint starts": "Χρησιμοποιείται στο παράθυρο του Glint, στην επικάλυψη κλιπ και στο μενού της περιοχής ειδοποιήσεων. Το μενού αλλάζει στην επόμενη εκκίνηση του Glint",
  "Hide the sidebar": "Απόκρυψη πλαϊνής στήλης",
  "Show the sidebar": "Εμφάνιση πλαϊνής στήλης",
  "Version": "Έκδοση",
  "Compact": "Συμπαγής",
  "Shows only the two stars in a small square, with no text. Problems, such as a clip that failed, still show the full box": "Δείχνει μόνο τα δύο αστέρια σε ένα μικρό τετράγωνο, χωρίς κείμενο. Τα προβλήματα, όπως ένα κλιπ που απέτυχε, εμφανίζονται στο πλήρες πλαίσιο",
});
