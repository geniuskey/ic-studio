; Metal frame: a 0.5 um metal1 ring, 1 um outside the bounding box of the edit cell.
; An example IC Studio extension without a program: one SKILL script as a menu command.
let((cv box x0 y0 x1 y1 g w lpp)
  cv = geGetEditCellView()
  box = cv~>bBox
  g = 1.0
  w = 0.5
  lpp = list("metal1" "drawing")
  x0 = xCoord(car(box)) - g - w
  y0 = yCoord(car(box)) - g - w
  x1 = xCoord(cadr(box)) + g + w
  y1 = yCoord(cadr(box)) + g + w
  dbCreateRect(cv lpp list(x0:y0 x1:y0+w))
  dbCreateRect(cv lpp list(x0:y1-w x1:y1))
  dbCreateRect(cv lpp list(x0:y0+w x0+w:y1-w))
  dbCreateRect(cv lpp list(x1-w:y0+w x1:y1-w))
  printf("metal frame around %s\n" cv~>cellName)
)
