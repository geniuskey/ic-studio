;; icsDump.il: dump a Virtuoso library for IC Studio (`ics import-virtuoso`).
;;
;; Load it in the CIW, then dump a library (or some of its cells):
;;   load("icsDump.il")
;;   icsDumpLib("myLib" "/tmp/myLib.icsdump")
;;   icsDumpLib("myLib" "/tmp/amp.icsdump" ?cells list("amp" "bias"))
;;   icsDumpLib("myLib" "/tmp/amp.icsdump" ?views list("schematic"))
;;
;; Schematic views: terminals with their directions, the cell's CDF parameter defaults, and
;; every instance with its master, placement, CDF parameter values and the net on each of its
;; terminals. Layout views: rectangles, polygons, paths, labels, instances and mosaics. Layout
;; masters outside the dumped cells (standard cells, PCell variants) are dumped as well, so the
;; hierarchy is complete; a PCell variant becomes a cell named <cell>_pc<n>.
;;
;; The file is one SKILL list per line: ("sch" lib cell), ("lay" lib cell dbuPerUU) and the
;; records of that view after it, ending with ("end").

procedure(icsWrite(p x)
  cond(
    (null(x) fprintf(p "()"))
    (stringp(x) fprintf(p "%L" x))
    (fixp(x) fprintf(p "%d" x))
    (floatp(x) fprintf(p "%.10g" x))
    (symbolp(x) fprintf(p "%L" get_pname(x)))
    (listp(x)
      fprintf(p "(")
      foreach(e x
        icsWrite(p e)
        fprintf(p " "))
      fprintf(p ")"))
    (t fprintf(p "%L" sprintf(nil "%L" x)))))

procedure(icsW(p x)
  icsWrite(p x)
  fprintf(p "\n"))

;; A property or CDF value as a string.
procedure(icsStr(v)
  cond(
    (null(v) "")
    (stringp(v) v)
    (fixp(v) sprintf(nil "%d" v))
    (floatp(v) sprintf(nil "%.10g" v))
    (symbolp(v) get_pname(v))
    (t sprintf(nil "%L" v))))

;; (name value) of every CDF parameter of an instance (its properties when it has no CDF).
procedure(icsInstParams(inst)
  let((cdf out)
    cdf = car(errset(cdfGetInstCDF(inst)))
    if(cdf
      then
        foreach(par cdf~>parameters
          when(par~>name && par~>paramType != "button"
            out = cons(list(par~>name icsStr(par~>value)) out)))
      else
        foreach(pr inst~>prop
          out = cons(list(pr~>name icsStr(pr~>value)) out)))
    reverse(out)))

procedure(icsDumpSch(p cv)
  let((cdf)
    icsW(p list("sch" cv~>libName cv~>cellName))
    foreach(term cv~>terminals
      icsW(p list("term" term~>name term~>direction)))
    cdf = car(errset(cdfGetBaseCellCDF(ddGetObj(cv~>libName cv~>cellName))))
    when(cdf
      foreach(par cdf~>parameters
        when(par~>name
          icsW(p list("cdf" par~>name icsStr(par~>defValue))))))
    foreach(inst cv~>instances
      icsW(p list("inst" inst~>name inst~>libName inst~>cellName inst~>viewName
                  inst~>xy inst~>orient or(inst~>numInst 1)
                  icsInstParams(inst)
                  foreach(mapcar it inst~>instTerms
                    list(it~>name if(it~>net it~>net~>name ""))))))))

;; Masters are keyed by lib/cell/view, PCell variants by the variant itself.
procedure(icsKey(m)
  if(m~>superMaster
    then m
    else sprintf(nil "%s/%s/%s" m~>libName m~>cellName m~>viewName)))

;; The (lib cell) a layout master is written under; queues it for dumping on first use.
procedure(icsMasterName(m)
  let((key name)
    key = icsKey(m)
    name = icsMasters[key]
    unless(name
      if(m~>superMaster
        then
          icsPcellCount = icsPcellCount + 1
          name = list(m~>libName sprintf(nil "%s_pc%d" m~>cellName icsPcellCount))
        else
          name = list(m~>libName m~>cellName))
      icsMasters[key] = name
      icsQueue = cons(m icsQueue))
    name))

procedure(icsDumpLay(p cv name)
  let((n i0)
    icsW(p list("lay" car(name) cadr(name) cv~>DBUPerUU))
    foreach(s cv~>shapes
      case(s~>objType
        ("rect" icsW(p list("rect" s~>layerName s~>purpose s~>bBox)))
        ("polygon" icsW(p list("poly" s~>layerName s~>purpose s~>points)))
        ("path" icsW(p list("path" s~>layerName s~>purpose s~>width s~>points)))
        ("pathSeg" icsW(p list("path" s~>layerName s~>purpose s~>width list(s~>beginPt s~>endPt))))
        ("label" icsW(p list("label" s~>layerName s~>purpose s~>theLabel s~>xy s~>height)))
        (t icsW(p list("skip" s~>objType s~>layerName s~>purpose)))))
    foreach(inst cv~>instances
      when(inst~>objType == "inst"
        n = if(inst~>master icsMasterName(inst~>master) list(inst~>libName inst~>cellName))
        icsW(p list("linst" inst~>name car(n) cadr(n) inst~>xy inst~>orient))))
    foreach(m cv~>mosaics
      i0 = car(m~>instanceList)
      when(i0 && i0~>master
        n = icsMasterName(i0~>master)
        icsW(p list("mosaic" m~>name car(n) cadr(n) m~>xy or(m~>orient i0~>orient "R0")
                    m~>rows m~>columns m~>uX m~>uY))))))

procedure(icsDumpLib(lib file @key (cells nil) (views list("schematic" "layout")))
  let((p dd cv m key count)
    dd = ddGetObj(lib)
    unless(dd error("icsDumpLib: no library %s" lib))
    p = outfile(file "w")
    unless(p error("icsDumpLib: cannot write %s" file))
    icsMasters = makeTable("icsMasters" nil)
    icsDone = makeTable("icsDone" nil)
    icsQueue = nil
    icsPcellCount = 0
    count = 0
    icsW(p list("icsdump" 1 lib))
    foreach(c or(cells dd~>cells~>name)
      when(member("schematic" views) && ddGetObj(lib c "schematic")
        cv = dbOpenCellViewByType(lib c "schematic" nil "r")
        when(cv
          icsDumpSch(p cv)
          count = count + 1))
      when(member("layout" views) && ddGetObj(lib c "layout")
        cv = dbOpenCellViewByType(lib c "layout" nil "r")
        when(cv
          key = icsKey(cv)
          icsMasters[key] = list(lib c)
          icsDone[key] = t
          icsDumpLay(p cv list(lib c))
          count = count + 1)))
    while(icsQueue
      m = car(icsQueue)
      icsQueue = cdr(icsQueue)
      key = icsKey(m)
      unless(icsDone[key]
        icsDone[key] = t
        icsDumpLay(p m icsMasters[key])
        count = count + 1))
    icsW(p list("end"))
    close(p)
    printf("IC Studio: wrote %d view(s) of %s to %s\n" count lib file)
    t))
